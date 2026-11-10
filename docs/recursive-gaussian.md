# Recursive Gaussian attempt (withheld in sync #312)

The former fork process 5 selected a variance-calibrated recursive CPU Gaussian,
while upstream process 5 means highlight roll-off. Upstream owns its ordinary
sequence numbers. Existing fork-only process 5 files are disposable fixtures and
benchmark outputs, so no migration is provided.

The sync first re-applied the CPU Gaussian as `ForkGaussianV1` in the separate
high-bit namespace (`2147483649` / `0x80000001`). Real GPU HDR tests rejected it:

| Case | Mean error | Maximum error |
|---|---:|---:|
| Fork HDR typical | 0.0863 | 10.005 |
| Fork HDR banded | 0.0865 | 19.923 |
| Upstream V7 HDR typical | 0.0002 | 0.052 |

Errors are measured in 8-bit sRGB steps on unbounded float samples. Upstream's
HDR limits are mean <0.02 and maximum ≤1 step. Its V1–V7 cases passed. The GPU
uses the three-box approximation, which differs from the CPU recursive kernel.

The incompatible render change is therefore removed in this sync. The upstream
Gaussian, V1–V7 fixtures, and full SDR/HDR parity gates are restored; HDR GPU
rendering remains enabled. Independent exact median, box-filter and startup I/O
optimisations are retained. The startup performance contracts still gate the sync.

The attempted implementation and fixtures are preserved on
[`failed/312-lightcraft-recursive-hdr-parity`](https://github.com/speedcraft-labs/lightcraft/tree/failed/312-lightcraft-recursive-hdr-parity).
Raw results are recorded in [hyper #312](https://github.com/speedcraft-labs/hyper/issues/312)
and [hyper #315](https://github.com/speedcraft-labs/hyper/issues/315).
A future reintroduction must use the high-bit namespace and supply a GPU path
that meets the parity bounds, before it can become the default render process.
