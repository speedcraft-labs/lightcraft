//! Exact separable median filters for fixed-width integer images.
//!
//! The two histogram levels make the 16-bit rank query bounded by 512 counter reads,
//! independent of the radius. Edges use replication, matching the float pipeline filter.

use crate::Image;

/// Horizontal then vertical median over `2 * radius + 1` samples, with replicated edges.
pub fn median_u8_hv(image: &Image<u8>, radius: usize) -> Image<u8> {
    if radius == 0 || image.width == 0 || image.height == 0 {
        return image.clone();
    }
    let horizontal = map_lines(image, false, |line| median_u8_line(line, radius));
    map_lines(&horizontal, true, |line| median_u8_line(line, radius))
}

/// Horizontal then vertical median over `2 * radius + 1` samples, with replicated edges.
pub fn median_u16_hv(image: &Image<u16>, radius: usize) -> Image<u16> {
    if radius == 0 || image.width == 0 || image.height == 0 {
        return image.clone();
    }
    let horizontal = map_lines_u16(image, false, radius);
    map_lines_u16(&horizontal, true, radius)
}

fn map_lines<T: Copy + Default>(image: &Image<T>, vertical: bool, f: impl Fn(&[T]) -> Vec<T>) -> Image<T> {
    let mut out = Image::new(image.width, image.height);
    let line_count = if vertical { image.width } else { image.height };
    let line_len = if vertical { image.height } else { image.width };
    let mut line = Vec::with_capacity(line_len);
    for n in 0..line_count {
        line.clear();
        for i in 0..line_len {
            line.push(if vertical { image.get(n, i) } else { image.get(i, n) });
        }
        let filtered = f(&line);
        for (i, &value) in filtered.iter().enumerate() {
            if vertical {
                out.set(n, i, value);
            } else {
                out.set(i, n, value);
            }
        }
    }
    out
}

fn median_u8_line(src: &[u8], radius: usize) -> Vec<u8> {
    let mut out = vec![0; src.len()];
    let mut low = [0u32; 256];
    let mut high = [0u32; 16];
    let add = |value: u8, increment: bool, low: &mut [u32; 256], high: &mut [u32; 16]| {
        let v = value as usize;
        if increment {
            low[v] += 1;
            high[v >> 4] += 1;
        } else {
            low[v] -= 1;
            high[v >> 4] -= 1;
        }
    };
    for i in 0..=radius.saturating_mul(2) {
        add(src[i.saturating_sub(radius).min(src.len() - 1)], true, &mut low, &mut high);
    }
    for (x, dst) in out.iter_mut().enumerate() {
        *dst = select_u8(&low, &high, radius);
        add(src[x.saturating_sub(radius)], false, &mut low, &mut high);
        add(src[x.saturating_add(radius).saturating_add(1).min(src.len() - 1)], true, &mut low, &mut high);
    }
    out
}

fn select_u8(low: &[u32; 256], high: &[u32; 16], rank: usize) -> u8 {
    let mut before = 0u32;
    let mut prefix = 0;
    for (value, &count) in high.iter().enumerate() {
        if before + count > rank as u32 {
            prefix = value << 4;
            break;
        }
        before += count;
    }
    for value in prefix..prefix + 16 {
        before += low[value];
        if before > rank as u32 {
            return value as u8;
        }
    }
    255
}

fn map_lines_u16(image: &Image<u16>, vertical: bool, radius: usize) -> Image<u16> {
    let mut out = Image::new(image.width, image.height);
    let line_count = if vertical { image.width } else { image.height };
    let line_len = if vertical { image.height } else { image.width };
    let mut line = Vec::with_capacity(line_len);
    let mut histogram = U16Histogram::new(line_len);
    for n in 0..line_count {
        line.clear();
        for i in 0..line_len {
            line.push(if vertical { image.get(n, i) } else { image.get(i, n) });
        }
        let filtered = histogram.filter(&line, radius);
        for (i, &value) in filtered.iter().enumerate() {
            if vertical {
                out.set(n, i, value);
            } else {
                out.set(i, n, value);
            }
        }
    }
    out
}

struct U16Histogram {
    lows: Vec<u32>,
    highs: [u32; 256],
    touched: Vec<u16>,
    touched_high: [bool; 256],
}

impl U16Histogram {
    fn new(line_len: usize) -> Self {
        Self { lows: vec![0; 65536], highs: [0; 256], touched: Vec::with_capacity(line_len.min(65536)), touched_high: [false; 256] }
    }

    fn add(&mut self, value: u16, increment: bool) {
        let v = value as usize;
        let hi = v >> 8;
        if increment && self.lows[v] == 0 {
            self.touched.push(value);
        }
        if increment && !self.touched_high[hi] {
            self.touched_high[hi] = true;
        }
        if increment {
            self.lows[v] += 1;
            self.highs[hi] += 1;
        } else {
            self.lows[v] -= 1;
            self.highs[hi] -= 1;
        }
    }

    fn filter(&mut self, src: &[u16], radius: usize) -> Vec<u16> {
        let mut out = vec![0; src.len()];
        for i in 0..=radius.saturating_mul(2) {
            self.add(src[i.saturating_sub(radius).min(src.len() - 1)], true);
        }
        for (x, dst) in out.iter_mut().enumerate() {
            *dst = select_u16(&self.lows, &self.highs, radius);
            self.add(src[x.saturating_sub(radius)], false);
            self.add(src[x.saturating_add(radius).saturating_add(1).min(src.len() - 1)], true);
        }
        for value in self.touched.drain(..) {
            self.lows[value as usize] = 0;
        }
        for (i, was_touched) in self.touched_high.iter_mut().enumerate() {
            if *was_touched {
                self.highs[i] = 0;
                *was_touched = false;
            }
        }
        out
    }
}

fn select_u16(lows: &[u32], highs: &[u32; 256], rank: usize) -> u16 {
    let mut before = 0;
    let mut high = 0;
    for (i, &count) in highs.iter().enumerate() {
        if before + count > rank as u32 {
            high = i;
            break;
        }
        before += count;
    }
    let base = high << 8;
    for low in 0..256 {
        before += lows[base + low];
        if before > rank as u32 {
            return (base + low) as u16;
        }
    }
    u16::MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle<T: Copy + Default + Ord>(src: &Image<T>, radius: usize) -> Image<T> {
        let pass = |image: &Image<T>, vertical: bool| {
            let mut out = image.clone();
            let count = if vertical { image.width } else { image.height };
            let len = if vertical { image.height } else { image.width };
            for n in 0..count {
                for i in 0..len {
                    let mut window = Vec::with_capacity(2 * radius + 1);
                    for k in 0..=2 * radius {
                        let p = k.saturating_add(i).saturating_sub(radius).min(len - 1);
                        window.push(if vertical { image.get(n, p) } else { image.get(p, n) });
                    }
                    window.sort_unstable();
                    if vertical {
                        out.set(n, i, window[radius]);
                    } else {
                        out.set(i, n, window[radius]);
                    }
                }
            }
            out
        };
        pass(&pass(src, false), true)
    }

    #[test]
    fn integer_histograms_match_brute_force_for_random_and_clamped_windows() {
        for (w, h) in [(1, 1), (2, 7), (17, 9), (65, 3)] {
            let a = Image::from_fn(w, h, |x, y| {
                (x as u32).wrapping_mul(2_654_435_761).wrapping_add((y as u32).wrapping_mul(2_246_822_519)).wrapping_add((x * y) as u32) as u8
            });
            let b = Image::from_fn(w, h, |x, y| {
                (x as u32).wrapping_mul(2_654_435_761).wrapping_add((y as u32).wrapping_mul(2_246_822_519)).wrapping_add((x * y) as u32) as u16
            });
            for radius in [0, 1, 3, 24] {
                assert_eq!(median_u8_hv(&a, radius), oracle(&a, radius), "u8 {w}x{h} r={radius}");
                assert_eq!(median_u16_hv(&b, radius), oracle(&b, radius), "u16 {w}x{h} r={radius}");
            }
        }
    }

    #[test]
    fn integer_histograms_match_pngsuite_corpus_when_available() {
        let Some(root) = std::env::var_os("ARTCRAFT_HYPER_ROOT").map(std::path::PathBuf::from) else { return };
        let corpus = root.join("corpus/pngsuite");
        let decoded8 = image::open(corpus.join("basn0g08.png"));
        assert!(decoded8.is_ok(), "8-bit PNGsuite corpus image must decode: {decoded8:?}");
        let Ok(decoded8) = decoded8 else { return };
        let image8 = decoded8.to_luma8();
        let a = Image { width: image8.width() as usize, height: image8.height() as usize, data: image8.into_raw() };
        let decoded16 = image::open(corpus.join("basn0g16.png"));
        assert!(decoded16.is_ok(), "16-bit PNGsuite corpus image must decode: {decoded16:?}");
        let Ok(decoded16) = decoded16 else { return };
        let image16 = decoded16.to_luma16();
        let b = Image { width: image16.width() as usize, height: image16.height() as usize, data: image16.into_raw() };
        for radius in [1, 3, 9, 24] {
            assert_eq!(median_u8_hv(&a, radius), oracle(&a, radius), "8-bit PNGsuite radius={radius}");
            assert_eq!(median_u16_hv(&b, radius), oracle(&b, radius), "16-bit PNGsuite radius={radius}");
        }
    }
}
