//! Exact separable median filters for fixed-width integer images.
//!
//! Running histograms and occupancy indexes keep window updates independent of the radius.
//! Edges use replication, matching the float pipeline filter.

use crate::Image;

/// Horizontal then vertical median over `2 * radius + 1` samples, with replicated edges.
pub fn median_u8_hv(image: &Image<u8>, radius: usize) -> Image<u8> {
    if radius == 0 || image.width == 0 || image.height == 0 {
        return image.clone();
    }
    let horizontal = median_u8_rows(image, radius);
    let vertical_input = transpose(&horizontal);
    transpose(&median_u8_rows(&vertical_input, radius))
}

/// Horizontal then vertical median over `2 * radius + 1` samples, with replicated edges.
pub fn median_u16_hv(image: &Image<u16>, radius: usize) -> Image<u16> {
    if radius == 0 || image.width == 0 || image.height == 0 {
        return image.clone();
    }
    // For short windows, moving the already-sorted taps is cheaper than maintaining a 16-bit
    // histogram. The histogram path keeps larger radii independent of the window width.
    let horizontal = if radius <= 32 { median_u16_sorted_rows(image, radius) } else { median_u16_rows(image, radius) };
    let vertical_input = transpose(&horizontal);
    let vertical = if radius <= 32 { median_u16_sorted_rows(&vertical_input, radius) } else { median_u16_rows(&vertical_input, radius) };
    transpose(&vertical)
}

fn median_u8_rows(image: &Image<u8>, radius: usize) -> Image<u8> {
    let mut out = Image::new(image.width, image.height);
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        out.data.par_chunks_mut(image.width).zip(image.data.par_chunks(image.width)).for_each(|(dst, src)| median_u8_line(src, dst, radius));
    }
    #[cfg(not(feature = "parallel"))]
    for (dst, src) in out.data.chunks_mut(image.width).zip(image.data.chunks(image.width)) {
        median_u8_line(src, dst, radius);
    }
    out
}

fn median_u8_line(src: &[u8], out: &mut [u8], radius: usize) {
    let mut histogram = U8Histogram::default();
    for i in 0..=radius.saturating_mul(2) {
        histogram.add(src[i.saturating_sub(radius).min(src.len() - 1)], true);
    }
    let (mut median, mut below) = histogram.select(radius);
    for (x, dst) in out.iter_mut().enumerate() {
        *dst = median;
        let old = src[x.saturating_sub(radius)];
        let new = src[x.saturating_add(radius).saturating_add(1).min(src.len() - 1)];
        histogram.slide(&mut median, &mut below, old, new, radius);
    }
}

struct U8Histogram {
    counts: [u32; 256],
    occupied: [u64; 4],
}

impl Default for U8Histogram {
    fn default() -> Self {
        Self { counts: [0; 256], occupied: [0; 4] }
    }
}

impl U8Histogram {
    fn add(&mut self, value: u8, increment: bool) {
        let value = value as usize;
        if increment {
            if self.counts[value] == 0 {
                self.occupied[value >> 6] |= 1u64 << (value & 63);
            }
            self.counts[value] += 1;
        } else {
            self.counts[value] -= 1;
            if self.counts[value] == 0 {
                self.occupied[value >> 6] &= !(1u64 << (value & 63));
            }
        }
    }

    fn select(&self, rank: usize) -> (u8, u32) {
        let mut before = 0u32;
        for (value, &count) in self.counts.iter().enumerate() {
            if before + count > rank as u32 {
                return (value as u8, before);
            }
            before += count;
        }
        (u8::MAX, before)
    }

    fn slide(&mut self, median: &mut u8, below: &mut u32, old: u8, new: u8, rank: usize) {
        if old == new {
            return;
        }
        if old < *median {
            *below -= 1;
        }
        self.add(old, false);
        if new < *median {
            *below += 1;
        }
        self.add(new, true);

        let count = self.counts[*median as usize];
        if *below > rank as u32 {
            *median = self.previous(*median);
            *below -= self.counts[*median as usize];
        } else if *below + count <= rank as u32 {
            *below += count;
            *median = self.next(*median);
        }
    }

    fn next(&self, value: u8) -> u8 {
        let word = value as usize >> 6;
        let bit = (value as usize & 63) + 1;
        if bit < 64 {
            let candidates = self.occupied[word] & (u64::MAX << bit);
            if candidates != 0 {
                return (word * 64 + candidates.trailing_zeros() as usize) as u8;
            }
        }
        for next_word in word + 1..self.occupied.len() {
            let candidates = self.occupied[next_word];
            if candidates != 0 {
                return (next_word * 64 + candidates.trailing_zeros() as usize) as u8;
            }
        }
        value
    }

    fn previous(&self, value: u8) -> u8 {
        let word = value as usize >> 6;
        let bit = value as usize & 63;
        let candidates = self.occupied[word] & bits_before(bit);
        if candidates != 0 {
            return (word * 64 + 63 - candidates.leading_zeros() as usize) as u8;
        }
        for previous_word in (0..word).rev() {
            let candidates = self.occupied[previous_word];
            if candidates != 0 {
                return (previous_word * 64 + 63 - candidates.leading_zeros() as usize) as u8;
            }
        }
        value
    }
}

fn median_u16_rows(image: &Image<u16>, radius: usize) -> Image<u16> {
    let mut out = Image::new(image.width, image.height);
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        out.data
            .par_chunks_mut(image.width)
            .zip(image.data.par_chunks(image.width))
            .for_each_init(|| U16Histogram::new(image.width), |histogram, (dst, src)| histogram.filter_into(src, dst, radius));
    }
    #[cfg(not(feature = "parallel"))]
    {
        let mut histogram = U16Histogram::new(image.width);
        for (dst, src) in out.data.chunks_mut(image.width).zip(image.data.chunks(image.width)) {
            histogram.filter_into(src, dst, radius);
        }
    }
    out
}

fn median_u16_sorted_rows(image: &Image<u16>, radius: usize) -> Image<u16> {
    let mut out = Image::new(image.width, image.height);
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        out.data.par_chunks_mut(image.width).zip(image.data.par_chunks(image.width)).for_each(|(dst, src)| median_u16_sorted_line(src, dst, radius));
    }
    #[cfg(not(feature = "parallel"))]
    for (dst, src) in out.data.chunks_mut(image.width).zip(image.data.chunks(image.width)) {
        median_u16_sorted_line(src, dst, radius);
    }
    out
}

fn median_u16_sorted_line(src: &[u16], out: &mut [u16], radius: usize) {
    let at = |i: isize| src[i.clamp(0, src.len() as isize - 1) as usize];
    let r = radius as isize;
    let mut window: Vec<u16> = (-r..=r).map(at).collect();
    window.sort_unstable();
    for (x, dst) in out.iter_mut().enumerate() {
        *dst = window[radius];
        let old = at(x as isize - r);
        let new = at(x as isize + r + 1);
        if old != new {
            let (mut i, mut j) = (0usize, 0usize);
            for &value in &window {
                i += usize::from(value < old);
                j += usize::from(value < new);
            }
            if j > i {
                window.copy_within(i + 1..j, i);
                window[j - 1] = new;
            } else {
                window.copy_within(j..i, j + 1);
                window[j] = new;
            }
        }
    }
}

fn transpose<T: Copy + Default>(image: &Image<T>) -> Image<T> {
    let mut out = Image::new(image.height, image.width);
    for y in 0..image.height {
        for x in 0..image.width {
            out.set(y, x, image.get(x, y));
        }
    }
    out
}

struct U16Histogram {
    lows: Vec<u32>,
    highs: [u32; 256],
    touched: Vec<u16>,
    touched_high: [bool; 256],
    occupied: Occupied16,
}

impl U16Histogram {
    fn new(line_len: usize) -> Self {
        Self {
            lows: vec![0; 65536],
            highs: [0; 256],
            touched: Vec::with_capacity(line_len.min(65536)),
            touched_high: [false; 256],
            occupied: Occupied16::new(),
        }
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
            if self.lows[v] == 1 {
                self.occupied.set(value);
            }
        } else {
            self.lows[v] -= 1;
            self.highs[hi] -= 1;
            if self.lows[v] == 0 {
                self.occupied.clear(value);
            }
        }
    }

    fn filter_into(&mut self, src: &[u16], out: &mut [u16], radius: usize) {
        for i in 0..=radius.saturating_mul(2) {
            self.add(src[i.saturating_sub(radius).min(src.len() - 1)], true);
        }
        let (mut median, mut below) = select_u16(&self.lows, &self.highs, radius);
        for (x, dst) in out.iter_mut().enumerate() {
            *dst = median;
            let old = src[x.saturating_sub(radius)];
            let new = src[x.saturating_add(radius).saturating_add(1).min(src.len() - 1)];
            self.slide(&mut median, &mut below, old, new, radius);
        }
        for value in self.touched.drain(..) {
            let count = &mut self.lows[value as usize];
            if *count != 0 {
                *count = 0;
                self.occupied.clear(value);
            }
        }
        for (i, was_touched) in self.touched_high.iter_mut().enumerate() {
            if *was_touched {
                self.highs[i] = 0;
                *was_touched = false;
            }
        }
    }

    fn slide(&mut self, median: &mut u16, below: &mut u32, old: u16, new: u16, rank: usize) {
        if old == new {
            return;
        }
        if old < *median {
            *below -= 1;
        }
        self.add(old, false);
        if new < *median {
            *below += 1;
        }
        self.add(new, true);

        let count = self.lows[*median as usize];
        if *below > rank as u32 {
            *median = self.occupied.previous(*median);
            *below -= self.lows[*median as usize];
        } else if *below + count <= rank as u32 {
            *below += count;
            *median = self.occupied.next(*median);
        }
    }
}

fn select_u16(lows: &[u32], highs: &[u32; 256], rank: usize) -> (u16, u32) {
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
        let count = lows[base + low];
        if before + count > rank as u32 {
            return ((base + low) as u16, before);
        }
        before += count;
    }
    (u16::MAX, before)
}

#[derive(Default)]
struct Occupied16 {
    values: Vec<u64>,
    words: Vec<u64>,
    top: u16,
}

impl Occupied16 {
    fn new() -> Self {
        Self { values: vec![0; 1024], words: vec![0; 16], top: 0 }
    }

    fn set(&mut self, value: u16) {
        let value = value as usize;
        let word = value >> 6;
        let bit = value & 63;
        if self.values[word] == 0 {
            let parent = word >> 6;
            if self.words[parent] == 0 {
                self.top |= 1 << parent;
            }
            self.words[parent] |= 1 << (word & 63);
        }
        self.values[word] |= 1 << bit;
    }

    fn clear(&mut self, value: u16) {
        let value = value as usize;
        let word = value >> 6;
        self.values[word] &= !(1 << (value & 63));
        if self.values[word] == 0 {
            let parent = word >> 6;
            self.words[parent] &= !(1 << (word & 63));
            if self.words[parent] == 0 {
                self.top &= !(1 << parent);
            }
        }
    }

    fn next(&self, value: u16) -> u16 {
        let value = value as usize;
        let word = value >> 6;
        let bit = (value & 63) + 1;
        if bit < 64 {
            let candidates = self.values[word] & (u64::MAX << bit);
            if candidates != 0 {
                return (word * 64 + candidates.trailing_zeros() as usize) as u16;
            }
        }
        let parent = word >> 6;
        let bit = (word & 63) + 1;
        if bit < 64 {
            let candidates = self.words[parent] & (u64::MAX << bit);
            if candidates != 0 {
                let next_word = parent * 64 + candidates.trailing_zeros() as usize;
                return (next_word * 64 + self.values[next_word].trailing_zeros() as usize) as u16;
            }
        }
        let bit = parent + 1;
        if bit < 16 {
            let candidates = self.top & (u16::MAX << bit);
            if candidates != 0 {
                let next_parent = candidates.trailing_zeros() as usize;
                let next_word = next_parent * 64 + self.words[next_parent].trailing_zeros() as usize;
                return (next_word * 64 + self.values[next_word].trailing_zeros() as usize) as u16;
            }
        }
        value as u16
    }

    fn previous(&self, value: u16) -> u16 {
        let value = value as usize;
        let word = value >> 6;
        let bit = value & 63;
        let candidates = self.values[word] & bits_before(bit);
        if candidates != 0 {
            return (word * 64 + 63 - candidates.leading_zeros() as usize) as u16;
        }
        let parent = word >> 6;
        let bit = word & 63;
        let candidates = self.words[parent] & bits_before(bit);
        if candidates != 0 {
            let previous_word = parent * 64 + 63 - candidates.leading_zeros() as usize;
            return (previous_word * 64 + 63 - self.values[previous_word].leading_zeros() as usize) as u16;
        }
        let candidates = self.top & bits_before16(parent);
        if candidates != 0 {
            let previous_parent = 15 - candidates.leading_zeros() as usize;
            let previous_word = previous_parent * 64 + 63 - self.words[previous_parent].leading_zeros() as usize;
            return (previous_word * 64 + 63 - self.values[previous_word].leading_zeros() as usize) as u16;
        }
        value as u16
    }
}

fn bits_before(bit: usize) -> u64 {
    if bit == 0 { 0 } else { u64::MAX >> (64 - bit) }
}

fn bits_before16(bit: usize) -> u16 {
    if bit == 0 { 0 } else { u16::MAX >> (16 - bit) }
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
            for radius in [0, 1, 3, 24, 33, 64] {
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
        for radius in [1, 3, 9, 24, 33] {
            assert_eq!(median_u8_hv(&a, radius), oracle(&a, radius), "8-bit PNGsuite radius={radius}");
            assert_eq!(median_u16_hv(&b, radius), oracle(&b, radius), "16-bit PNGsuite radius={radius}");
        }
    }
}
