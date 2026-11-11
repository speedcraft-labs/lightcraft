# Rejected UI-dispatched record reads (313b)

One complete diagnostic pair of identical-source clean builds passed all output
and resource guards. Warm switch app medians were60.08/87.27ms; preview97.77/97.66,
exposure6.96/6.94 and export323.37/324.97. N=1 has no valid precision inference.

Source review found that ui.latency.wait still entered the generic UI dispatch /
reply wait. It forced a frame to read the record and requested further repaints
at the generic wait interval (500ms). This makes recorder reads perturb the UI
whose natural response is being measured. The diagnostic differences do NOT
prove that this caused their variance; the read-side effect itself is rejected.

Correction: expose a thread-safe read-only recorder handle to the socket thread.
GPU callbacks complete its response without any UI request or repaint. A server
regression verifies zero dispatched UI requests and zero repaint callbacks;
a recorder test completes a reader registered on a separate thread. All compared
builds receive one revised common patch, and the20-pair campaign restarts fresh.

Raw report: keep/313b-inapp-timing/aa-inapp-r2-0/report.json. No successful
observation is trimmed; the entire prior method is diagnostic and excluded.
