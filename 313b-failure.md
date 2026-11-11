# Rejected startup endpoint (313b)

Development smoke, same fork main d97557e0: startup recorder ended at 452.917 ms
while the first verified full-photo external frame arrived at 1579.328 ms.
The recorder incorrectly treated a filmstrip thumbnail as a completed Detail
view. All five action records and pixel checks completed, but this startup
endpoint is rejected and the smoke is excluded from inference.

Fix: Detail startup requires the active loupe's full render (including any
pending region), with first response separately recorded. A regression test
places a filmstrip texture in the renderer while the main loupe is absent.
Raw evidence: keep/313b-inapp-timing/smoke-01 and smoke-01-pixel-checks.json.
