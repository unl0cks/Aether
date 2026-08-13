A pre-release. The buff icons, the drop-accept button and the aura bar text should sit where they
belong again.

## Slicing now declines the two cases that were moving things

0.6.8 stopped a **cached** object being sliced, which is a real fault and is still fixed. It was not
the only one. Two more, both of which moved exactly the things that were reported:

**An object placed smaller than it was drawn.** A border is kept at its drawn size by dividing it by
the object's scale. Below one, that division makes the border band *larger* than it was drawn, and
the piece covering the corner magnifies whatever it reaches -- a sliver of the object's own interior,
smeared across its own corner. That is the dark wedge at the top left of the buff icons and the
drop-accept button, both of which are placed smaller than they were drawn. Nothing is lost by
declining: a grid exists to stop corners stretching as a panel grows, and a panel that is not
growing has no corners being stretched.

**An object holding anything that was positioned rather than drawn.** Slicing redraws each band
under its own transform. That is right for art and wrong for a caption, a stack count, an icon or a
button: one sitting in a border band is redrawn at its authored size anchored to the object's edge,
which is to say moved up and to the left.

Counted on the live build, `cargo run -p swf --example scaling_grid_census`: of its **80 scaling
grids, 79 sit over a single shape and nothing else** -- panel backgrounds and Flash's own component
skins. So declining on anything else costs one object in eighty and protects every icon, count and
label in the game.

Measured on a bordered box at three times its size, and at half:

| case | border | contents at | same as unsliced? |
|---|---|---|---|
| art, grown | **12 px** | 72, 72 | no -- sliced, which is the point |
| art, grown, no grid | 36 px | 96, 96 | -- |
| holds a positioned child, grown | 36 px | 96, 96 | **yes, pixel for pixel** |
| art, shrunk | 6 px | 66, 66 | **yes, pixel for pixel** |

## Text

Still not fixed, and it is the next thing. What is now ruled out: it is not the cache losing
resolution, because the stage's own scaling is applied before an object is measured, so a cached
panel is stored at the size it appears on screen rather than the movie's. It is not the texture size
rounding, which only ever rounds up. Antialiasing follows the renderer's default rather than being
forced low. That leaves how the glyphs themselves are filled, against a client running real Flash
for comparison.

## From 0.6.8

**Cached objects are no longer sliced.** A cached object -- one already drawn once and kept as a
picture, which is every panel wearing a drop shadow -- ignores a transform's scale when it is drawn
back, because the picture already has its size baked in. It does not ignore the shift. So the shift
arrived on its own, nine times over.

**The movement stop guard runs for the first time.** It decided whether a method was `walkTo` or
`stopWalking` by reading the method's name out of the file. That name is optional, and AQW does not
write it: measured on the live build, all 5,605 of its methods are nameless, while the staging build
`spider.swf` names every one of its 5,436. Every check made against the staging file said the hook
was fine while it had never once fired for a player. It reads the trait name now, which is what a
decompiler reads and what stripping cannot remove.

## Downloads

`Aether-Setup-0.6.9-win-x64.exe` for the installer, `Aether-Portable-0.6.9-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.9-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
