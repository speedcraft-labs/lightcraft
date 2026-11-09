//! Separable Gaussian blur: variance-exact sampled windows below 2.5 px, otherwise a
//! third-order Young–van Vliet recursive filter with Triggs–Sdika clamp boundaries.
//! The unversioned [`gaussian`] API retains upstream's three-box approximation.
//! [`gaussian_recursive`] is selected explicitly by the fork render process.

use crate::resample::Pixel;
use crate::{Image, par_rows};

/// The largest blur sigma (pixels) the box radii are made for. A box wider than any image (the
/// encoders' limit is 65535 px) blurs to the same flat result, and a larger sigma made the box
/// width's square overflow (from about 23 170 px: a panic where overflow is checked, a garbage
/// radius of billions of pixels, i.e. a hung render, where it isn't).
pub const MAX_SIGMA: f32 = 65_536.0;

/// A normalized sampled Gaussian whose discrete variance is `sigma²`.
///
/// Returns `None` for non-finite, non-positive or greater-than-33-pixel sigmas, bounding its cost.
pub fn variance_exact_kernel(sigma: f32) -> Option<Vec<f32>> {
    if !sigma.is_finite() || sigma <= 0.0 || sigma > 33.0 {
        return None;
    }
    let half = (3.0 * sigma).ceil() as i32;
    let target = f64::from(sigma).powi(2);
    let sample = |width: f64| {
        let mut weights: Vec<f64> = (-half..=half).map(|offset| (-f64::from(offset * offset) / (2.0 * width * width)).exp()).collect();
        let sum: f64 = weights.iter().sum();
        weights.iter_mut().for_each(|weight| *weight /= sum);
        weights
    };
    let (mut low, mut high) = (f64::from(sigma) * 0.01, f64::from(sigma) * 2.0 + 1.0);
    for _ in 0..48 {
        let middle = (low + high) * 0.5;
        let variance: f64 = sample(middle).iter().enumerate().map(|(index, weight)| weight * (index as f64 - f64::from(half)).powi(2)).sum();
        if variance < target {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(sample((low + high) * 0.5).into_iter().map(|weight| weight as f32).collect())
}

/// Box radii approximating a Gaussian of `sigma` with 3 passes (Kovesi / Wells). A sigma that is
/// not a number counts as 0 (no blur); one above [`MAX_SIGMA`] (infinity too) as `MAX_SIGMA`.
pub fn box_radii(sigma: f32) -> [usize; 3] {
    let sigma = if sigma.is_nan() { 0.0 } else { sigma.clamp(0.0, MAX_SIGMA) };
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    // (the square in i64: it passes i32 from a box 46 341 px wide)
    let m_ideal = (12.0 * sigma * sigma - n * (i64::from(wl) * i64::from(wl)) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round() as i32;
    std::array::from_fn(|i| (((if (i as i32) < m { wl } else { wu }) - 1) / 2).max(0) as usize)
}

/// Standard deviation (px) of the blur [`gaussian_box`] actually makes for `sigma`: its boxes are
/// whole pixels wide, so small radii snap to a few steps (0 below about 0.58 px, then √(2/3) px for one
/// 3-px box per axis, …).
pub fn box_sigma(sigma: f32) -> f32 {
    if sigma.is_nan() || sigma <= 0.3 {
        return 0.0;
    }
    box_radii(sigma).iter().map(|&r| (((2 * r + 1) * (2 * r + 1) - 1) as f32) / 12.0).sum::<f32>().sqrt()
}

/// One horizontal box pass over `row` (clamped edges), using `tmp` as scratch.
#[inline]
fn box_row<T: Pixel>(row: &mut [T], tmp: &mut Vec<T>, r: usize) {
    let w = row.len();
    if r == 0 || w == 0 {
        return;
    }
    tmp.clear();
    tmp.extend_from_slice(row);
    let s = &tmp[..];
    let last = w - 1;
    let inv = 1.0 / (2 * r + 1) as f32;
    let mut acc = T::zero();
    for i in 0..=2 * r {
        acc = acc.madd(s[i.saturating_sub(r).min(last)], 1.0);
    }
    for x in 0..w {
        row[x] = T::zero().madd(acc, inv);
        acc = acc.madd(s[(x + r + 1).min(last)], 1.0).madd(s[x.saturating_sub(r)], -1.0);
    }
}

/// Rows per parallel band in the vertical pass (each band re-primes its running sums).
pub const BAND: usize = 32;

/// One vertical box pass `src → dst` (clamped edges): a running sum per column, vectorized across
/// the row, parallel over horizontal bands. Cache-friendly — no transposes.
fn box_v<T: Pixel>(src: &Image<T>, dst: &mut Image<T>, r: usize) {
    let (w, h) = (src.width, src.height);
    let inv = 1.0 / (2 * r + 1) as f32;
    let last = h - 1;
    let row = |y: usize| &src.data[y * w..(y + 1) * w];
    par_rows(&mut dst.data, w * BAND, |band, out| {
        let y0 = band * BAND;
        let mut acc = vec![T::zero(); w];
        for i in 0..=2 * r {
            let yy = (y0 + i).saturating_sub(r).min(last);
            for (a, v) in acc.iter_mut().zip(row(yy)) {
                *a = a.madd(*v, 1.0);
            }
        }
        for (k, o) in out.chunks_mut(w).enumerate() {
            let y = y0 + k;
            for (d, a) in o.iter_mut().zip(&acc) {
                *d = T::zero().madd(*a, inv);
            }
            let (add, sub) = (row((y + r + 1).min(last)), row(y.saturating_sub(r)));
            for ((a, p), m) in acc.iter_mut().zip(add).zip(sub) {
                *a = a.madd(*p, 1.0).madd(*m, -1.0);
            }
        }
    });
}

/// The box radii of [`gaussian_box`] on a `w × h` image (the GPU renderer's twin uses them too), or
/// `None` when the blur leaves the image as it is: `sigma` at most 0.3 or not a number, or an
/// empty image. A sigma wider than twice the image's long side counts as that: a crop a hair
/// wide shown to fit asked the tone and dehaze stages for sigmas of millions of pixels (infinite
/// for a crop of no size), which blur no flatter than that but cost boxes that wide on every row.
pub fn radii_for(sigma: f32, w: usize, h: usize) -> Option<[usize; 3]> {
    if sigma.is_nan() || sigma <= 0.3 || w == 0 || h == 0 {
        return None;
    }
    let cap = (w.max(h) as f32 * 2.0).max(1.0);
    Some(box_radii(sigma.min(cap)))
}

/// Blur with Gaussian `sigma` (pixels). `sigma <= 0.3` returns a copy.
pub fn gaussian_box<T: Pixel>(img: &Image<T>, sigma: f32) -> Image<T> {
    let Some(radii) = radii_for(sigma, img.width, img.height) else {
        return img.clone();
    };
    let mut a = img.clone();
    par_rows(&mut a.data, img.width, |_, row| {
        let mut tmp = Vec::with_capacity(row.len());
        for r in radii {
            box_row(row, &mut tmp, r);
        }
    });
    let mut b = Image::<T>::new(img.width, img.height);
    for r in radii {
        if r == 0 {
            continue;
        }
        box_v(&a, &mut b, r);
        std::mem::swap(&mut a, &mut b);
    }
    a
}

/// Upstream's unversioned Gaussian API. Keep new upstream callers (including colour NR)
/// on their original kernel; only the fork render process opts into [`gaussian_recursive`].
pub fn gaussian<T: Pixel>(image: &Image<T>, sigma: f32) -> Image<T> {
    gaussian_box(image, sigma)
}

/// Apply the shared Young–van Vliet kernel to built-in floating pixel formats. `None` means an
/// unsupported layout/input and lets the caller retain the legacy custom-pixel path.
pub(crate) fn shared_image<const N: usize, T: Pixel>(
    image: &Image<T>,
    sigma: f32,
    widen: impl Fn(T) -> [f64; N],
    narrow: impl Fn([f64; N]) -> T,
) -> Option<Image<T>> {
    use craft_blur::{Cancel, Edge, Gaussian, Quality, Scratch, Shape};
    if image.width == 0 || image.height == 0 {
        return Some(image.clone());
    }
    let shape = Shape::new(image.width, image.height, N).ok()?;
    let filter = Gaussian::new(sigma, Quality::RecursiveRaw).ok()?;
    let mut pixels = Vec::with_capacity(shape.samples());
    for &pixel in &image.data {
        pixels.extend(widen(pixel).map(|value| value as f32));
    }
    filter.apply(&mut pixels, shape, Edge::Clamp, &mut Scratch::default(), Cancel::Never).ok()?;
    let mut out = Image::new(image.width, image.height);
    let (pixels, remainder) = pixels.as_chunks::<N>();
    debug_assert!(remainder.is_empty());
    out.data = pixels.iter().map(|values| narrow(std::array::from_fn(|i| f64::from(values[i])))).collect();
    Some(out)
}

/// Blur with the variance-calibrated sampled kernel at small sigma and the shared recursive
/// Gaussian otherwise. Legacy saved renders stay on [`gaussian_box`] through their process version.
pub fn gaussian_recursive<T: Pixel>(image: &Image<T>, sigma: f32) -> Image<T> {
    if !sigma.is_finite() || sigma <= 0.0 || image.width == 0 || image.height == 0 {
        return gaussian_box(image, sigma);
    }
    let sigma = sigma.min(MAX_SIGMA).min((image.width.max(image.height) as f32 * 2.0).max(1.0));
    T::gaussian_shared(image, sigma).unwrap_or_else(|| gaussian_box(image, sigma))
}

/// Largest sigma [`gauss_taps`] makes taps for (larger ones are clamped to it). The GPU blur reads
/// the same taps, so it is the limit of both paths.
pub const GAUSS_SIGMA_MAX: f32 = 64.0;

/// Normalized Gaussian taps `[w(-r) .. w(r)]` for `sigma`, `r = ceil(3 sigma)`.
///
/// An exact kernel, unlike the three-box approximation of [`gaussian`], which cannot represent
/// a sigma under about 0.8 px (its boxes collapse to the identity): sharpening needs those.
pub fn gauss_taps(sigma: f32) -> Vec<f32> {
    let sigma = if sigma.is_nan() { 0.05 } else { sigma.clamp(0.05, GAUSS_SIGMA_MAX) };
    let r = (3.0 * sigma).ceil() as i32;
    let k = -0.5 / (sigma * sigma);
    let mut t: Vec<f32> = (-r..=r).map(|i| ((i * i) as f32 * k).exp()).collect();
    let sum: f32 = t.iter().sum();
    t.iter_mut().for_each(|v| *v /= sum);
    t
}

/// Separable blur with odd-length symmetric `taps` (clamped edges), see [`gauss_taps`].
pub fn gaussian_taps<T: Pixel>(img: &Image<T>, taps: &[f32]) -> Image<T> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || taps.len() < 3 {
        return img.clone();
    }
    let r = (taps.len() / 2) as isize;
    let mut a = Image::<T>::new(w, h);
    par_rows(&mut a.data, w, |y, row| {
        let src = &img.data[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = T::zero();
            for (k, t) in taps.iter().enumerate() {
                let xi = (x as isize + k as isize - r).clamp(0, w as isize - 1) as usize;
                acc = acc.madd(src[xi], *t);
            }
            *o = acc;
        }
    });
    let mut b = Image::<T>::new(w, h);
    par_rows(&mut b.data, w, |y, row| {
        for (k, t) in taps.iter().enumerate() {
            let yi = (y as isize + k as isize - r).clamp(0, h as isize - 1) as usize;
            for (o, v) in row.iter_mut().zip(&a.data[yi * w..(yi + 1) * w]) {
                *o = o.madd(*v, *t);
            }
        }
    });
    b
}

#[cfg(test)]
mod tests {
    #[test]
    fn fir_gaussian_has_bounded_finite_taps_for_hostile_sigmas() {
        for sigma in [f32::NAN, f32::NEG_INFINITY, f32::INFINITY, f32::MAX, -5.0, 0.0, 0.1, 4.0] {
            let taps = super::gauss_taps(sigma);
            assert!(taps.len() <= 385 && taps.len() % 2 == 1);
            assert!(taps.iter().all(|v| v.is_finite() && *v >= 0.0));
            assert!((taps.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn exact_kernel_has_requested_variance() {
        for sigma in [0.3f32, 0.5, 0.7, 1.0, 1.5, 2.0, 4.0, 8.0, 16.0, 33.0] {
            let kernel = super::variance_exact_kernel(sigma).expect("valid sigma");
            let half = kernel.len() / 2;
            let sum: f32 = kernel.iter().sum();
            let variance: f32 = kernel.iter().enumerate().map(|(index, weight)| weight * (index as f32 - half as f32).powi(2)).sum();
            assert!((sum - 1.0).abs() < 1e-5, "sigma {sigma}: sum {sum}");
            assert!((variance / sigma.powi(2) - 1.0).abs() < 0.01, "sigma {sigma}: variance {variance}");
        }
        for sigma in [0.0, -1.0, f32::NAN, f32::INFINITY, 33.01] {
            assert!(super::variance_exact_kernel(sigma).is_none());
        }
    }

    /// The box radii of the formula as it was, for the check below.
    fn box_radii_before(sigma: f32) -> [usize; 3] {
        let n = 3.0f32;
        let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
        let mut wl = w_ideal.floor() as i32;
        if wl % 2 == 0 {
            wl -= 1;
        }
        let wu = wl + 2;
        let m_ideal = (12.0 * sigma * sigma - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0);
        let m = m_ideal.round() as i32;
        std::array::from_fn(|i| (((if (i as i32) < m { wl } else { wu }) - 1) / 2).max(0) as usize)
    }

    /// A blur sigma from a degenerate edit or a deep zoom (found by a UI campaign: a render
    /// worker's blur with a huge sigma) squared the box width past i32, which panics in a debug
    /// build and gave a radius of billions of pixels in a release one. Huge, infinite and NaN
    /// sigmas give bounded radii, and a blur with them is quick and stays finite; every sigma that
    /// worked before keeps exactly its radii.
    #[test]
    fn huge_and_non_finite_sigmas_give_bounded_radii() {
        for sigma in [26_800.0, 46_341.0, 1e5, 1e9, f32::MAX, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, -5.0] {
            let r = box_radii(sigma);
            assert!(r.iter().all(|&r| r <= MAX_SIGMA as usize * 2), "{sigma}: {r:?}");
        }
        assert_eq!(box_radii(f32::NAN), [0, 0, 0]);
        assert_eq!(box_radii(f32::NEG_INFINITY), [0, 0, 0]);
        assert_eq!(box_radii(1e9), box_radii(MAX_SIGMA));
        assert_eq!(box_radii(f32::INFINITY), box_radii(MAX_SIGMA));
        // on an image, the blur is at most twice its long side; NaN and tiny sigmas don't blur
        assert_eq!(radii_for(3.1e7, 1400, 900), Some(box_radii(2800.0)));
        assert_eq!(radii_for(f32::INFINITY, 1400, 900), Some(box_radii(2800.0)));
        assert_eq!(radii_for(40.0, 1400, 900), Some(box_radii(40.0)), "an ordinary sigma is as asked");
        assert_eq!(radii_for(f32::NAN, 1400, 900), None);
        assert_eq!(radii_for(0.2, 1400, 900), None);
        assert_eq!(radii_for(5.0, 0, 900), None);
        let p = crate::Plane { width: 4, height: 3, data: (0..12).map(|i| i as f32).collect() };
        let t = std::time::Instant::now();
        for sigma in [1e9, f32::INFINITY, f32::NAN] {
            let b = gaussian(&p, sigma);
            assert!(b.data.iter().all(|v| v.is_finite()), "{sigma}");
        }
        assert!(t.elapsed().as_secs_f64() < 30.0, "a huge sigma's blur is bounded");
        let mut s = 0.0f32;
        while s < 23_000.0 {
            assert_eq!(box_radii(s), box_radii_before(s), "sigma {s}");
            s = s * 1.013 + 0.07;
        }
    }

    use super::*;

    #[test]
    fn unversioned_gaussian_keeps_upstream_box_output_and_fork_is_explicit() {
        let mut impulse = Image::<f32>::new(101, 1);
        impulse.set(50, 0, 1.0);
        for sigma in [0.5, 1.0, 4.0, 8.0] {
            assert_eq!(gaussian(&impulse, sigma), gaussian_box(&impulse, sigma));
        }
        assert_ne!(gaussian_recursive(&impulse, 1.0), gaussian(&impulse, 1.0));
    }

    #[test]
    fn preserves_constant_and_mass() {
        let img = Image::<f32>::filled(50, 30, 0.7);
        let b = gaussian(&img, 4.0);
        assert!(b.data.iter().all(|v| (v - 0.7).abs() < 1e-5));
        let mut imp = Image::<f32>::new(101, 101);
        imp.set(50, 50, 1.0);
        let b = gaussian(&imp, 5.0);
        let sum: f32 = b.data.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
        // peak moved to the centre and symmetric
        assert!((b.get(45, 50) - b.get(55, 50)).abs() < 1e-6);
        assert!((b.get(50, 45) - b.get(50, 55)).abs() < 1e-6);
    }

    #[test]
    fn box_sigma_is_the_blur_made() {
        assert_eq!(box_sigma(0.3), 0.0);
        assert_eq!(box_sigma(0.5), 0.0, "no box wider than 1 px");
        assert!((box_sigma(1.0) - (2.0f32 / 3.0).sqrt()).abs() < 1e-6);
        for sigma in [1.0f32, 2.5, 8.0] {
            let mut imp = Image::<f32>::new(201, 1);
            imp.set(100, 0, 1.0);
            let b = gaussian_box(&imp, sigma);
            let var: f32 = b.data.iter().enumerate().map(|(i, v)| v * ((i as f32 - 100.0).powi(2))).sum();
            assert!((var.sqrt() - box_sigma(sigma)).abs() < 1e-3, "{sigma}: {} vs {}", var.sqrt(), box_sigma(sigma));
        }
    }

    #[test]
    fn variance_close_to_sigma_squared() {
        let mut imp = Image::<f32>::new(201, 1);
        imp.set(100, 0, 1.0);
        let sigma = 8.0;
        let b = gaussian(&imp, sigma);
        let var: f32 = b.data.iter().enumerate().map(|(i, v)| v * ((i as f32 - 100.0).powi(2))).sum();
        assert!((var.sqrt() - sigma).abs() / sigma < 0.1, "{}", var.sqrt());
    }
}
