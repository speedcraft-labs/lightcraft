# Recursive Gaussian (issue #176)

LightCraft process ForkGaussianV1 (`2147483649` / `0x80000001`) replaces the CPU three-box Gaussian with the shared
`craft-blur` filter. upstream V1–V7 keep the previous box output so existing saved edits
render as before. The GPU filter is unchanged.

## What changes

The old cascade chose three neighboring odd box widths. Their discrete variance
is the sum of each box's `(width² - 1) / 12`, so integer width changes make the
actual blur jump as sigma changes. The measured variance error can reach the
`1 / (3 sigma)` limit; at sigma 1 the three-box selection has variance `2/3`
instead of the requested `1`.

ForkGaussianV1 (`2147483649` / `0x80000001`) uses the normalized sampled Gaussian below sigma 2.5 and the variance
calibrated third-order Young–van Vliet recurrence from sigma 2.5 onward. The
recurrence refines its pole parameter to match the requested second moment and
uses Triggs–Sdika initialization for clamped boundaries. It carries f64 state;
edge pixels extend exactly as they did in the CPU box path. Empty images,
nonpositive and NaN sigmas retain the old copy/no-blur behavior, while very
large sigmas remain bounded by image size.

## Accuracy contract

`craft-blur` impulse and property tests compare the implementation with a
normalized, variance-calibrated sampled Gaussian. Across the tested sigma range
0.5–64, discrete variance error is below 0.5%. For raw YvV, maximum absolute
impulse shape error is 0.023721 at sigma 2.5 and 0.011705 at sigma 4; it is
below 0.01 from sigma 8 onward in the measured table. A 48-case generated-sigma
property test enforces a 0.03 maximum impulse-shape error on [2.5, 8), 0.01 on
[8, 64], and variance error below 0.5% across [2.5, 64]. Shape error is the
maximum absolute difference between unit-mass impulse taps, not a relative-tap
or arbitrary-image error bound. The shared crate documents its guard bands and
edge contracts in [`craft-blur`](https://github.com/speedcraft-labs/craft-libs/tree/algo/integration/crates/blur).

## Versioned paths and fixtures

The process-aware CPU blur paths, including noise reduction, texture, dehaze,
mask refinement, local adjustments and heal overlays, choose their blur by
process version. Process ForkGaussianV1 (`2147483649` / `0x80000001`) is included in the stage-cache key so switching
between V4 and ForkGaussianV1 (`2147483649` / `0x80000001`) recomputes affected planes. Unversioned helper APIs preserve
their legacy box behavior for callers that do not provide process settings.

No upstream V1–V7 golden changed. The before/after impulse test confirms that ForkGaussianV1 (`2147483649` / `0x80000001`) has the
corrected width while the legacy helper retains the old cascade. The
pixel-scale SharpenMask preview uses a separate sampled kernel and is unchanged.
The process-switch test checks cached and fresh renders with noise reduction,
local adjustments, a subject mask and a heal spot, then switches back to V4 to
confirm its output is restored.

## Upstream sync #312

The old fork process 5 is disposable test/benchmark data, per the maintainer.
There is no migration path. Upstream owns ordinary process numbers, including
V5 highlight roll-off, V6 joint colour NR and V7 crystal grain. New fork-only
render versions use the high-bit namespace starting at `0x80000001` to avoid
reusing upstream sequence numbers on each sync. ForkGaussianV1 builds on all
upstream V7 behavior and changes only the versioned CPU Gaussian selection.

The unversioned raster `gaussian` API keeps upstream's three-box output, including
new upstream joint colour NR callers. Only the fork process selects
`gaussian_recursive`; upstream V1–V7 do not inherit the fork kernel.
