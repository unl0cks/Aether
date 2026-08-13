A pre-release. Scaling grids are on for everyone again, and the movement stop guard runs for the
first time.

## The buttons, icons and text that moved

Slicing a panel into nine pieces draws it nine times, each piece under a transform that carries both
a stretch and a shift. A **cached** object -- one Flash has already drawn once and kept as a picture,
which is every panel wearing a drop shadow or a glow -- ignores the stretch when it is drawn back,
because the picture already has its size baked in. It does not ignore the shift.

So the shift arrived on its own, applied to an unscaled picture, nine times over. That is one cause
for all three reports: the drop-accept checkmark and the buff icons sitting up and to the left, the
text in the aura bar doing the same, and tall tooltips losing their rounded corners altogether.

The shift is `low x (1 - 1/scale)`, where `low` is the near edge of the object. It is exactly zero
when the art is drawn from the object's own origin outwards -- which is what the test case did, and
why it measured clean while the game did not. Redrawn centred on its origin instead, as AQW builds
its panels, at three times its size:

| | box starts at | middle starts at | border |
|---|---|---|---|
| no grid, cached | 60, 60 | 96, 96 | 36 px |
| grid, cached, before | 60, 60 | **60, 60** | **0 px, the border is gone** |
| grid, cached, now | 60, 60 | 96, 96 | 36 px |

A cached object is now left alone entirely, and comes out pixel for pixel what it was before any of
this existed. Panels that are not cached are still sliced, and still keep their borders: 12 pixels
at three times the size, not 36.

## Seams, and why they came back on a big window

The overlap that closes the seam between two pieces was measured in the object's own pixels rather
than the screen's. An object's own pixel is worth however much it has been stretched, so on a
tooltip pulled ten times over, half a pixel of overlap became five pixels of the middle painted
across the corner beside it -- which is enough to swallow a rounded corner whole. The taller the
tooltip, the squarer its corners came out.

It is measured in screen pixels now, so the overlap is half a pixel at any size.

The **Keep panel borders unstretched** switch is gone. It should not have been a switch.

## The movement stop guard has never once run

The guard that holds a walk together decides whether a method is `walkTo` or `stopWalking` by
reading the method's name out of the file. That name is optional, and a shipped build may simply not
write it.

AQW does not write it. Measured on both builds:

| build | methods | with no name |
|---|---|---|
| `spider.swf`, the staging build | 5,436 | 0 |
| `Game3098r24.swf`, the live build | 5,605 | **all 5,605** |

Every method in the game everyone actually plays is nameless, so the name being compared was always
the empty string and never matched anything. The staging build keeps its names, which is why every
check made against that file said the hook was fine.

This is why two movement traces came back with thousands of records and not one of them about
movement, and why widening the guard's scope never helped: it was never reached. A decompiler still
shows `walkTo` because it reads the *traits*, a separate table that always carries names. The
classifier reads the trait now, so the guard runs.

Whether it stops the premature stops is a separate question, and one that can finally be answered:
`--input-trace` will now record walks, and attribute every declined stop to the gate that declined
it.

## Text

Not fixed. What has been ruled out: it is not the cache losing resolution -- the stage's own scaling
is applied before an object is measured, so a cached panel is stored at the size it appears on
screen, not the movie's. It is not the texture size rounding, which only ever rounds up. Antialiasing
follows the renderer's default rather than being forced low. That leaves how the glyphs themselves
are filled, which is a different piece of work and is honestly still open.

## Two tools that stay

`cargo run -p swf --example abc_method_names -- <movie.swf>` reports whether a movie carries method
names, which is what caught the above.

`cargo run -p swf --example scaling_grid_census -- <movie.swf>` lists every scaling grid and what is
inside the object carrying it. On the live build: 80 grids, 79 of them over a single shape and
nothing else.

## Downloads

`Aether-Setup-0.6.8-win-x64.exe` for the installer, `Aether-Portable-0.6.8-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.8-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
