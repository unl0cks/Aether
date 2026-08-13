A pre-release. Border art and the things sitting on top of it are drawn separately now, so a panel
can keep its corners without moving its contents.

## Skill 3 and 4's corners, and the icons that moved

These were the same problem pulling in opposite directions, and 0.6.9 answered it the wrong way.

Slicing draws an object in nine pieces, each under its own transform. That is right for art, which
was **drawn**, and wrong for a caption, a stack count, an icon or a button, which were
**positioned**: one sitting in a border band gets redrawn at its authored size anchored to the
object's edge, which is to say moved up and to the left.

0.6.9 dealt with that by declining to slice anything holding such a child. That stopped the icons
moving and took the corners with them, which is what the third and fourth skill buttons were showing
-- their border is skinned by an object that also holds something positioned, so it stopped being
protected.

The two are drawn separately now. The art is sliced; everything else is drawn once, under the
object's ordinary transform and no cell mask, exactly where it would have been had none of this
happened. Measured on a bordered box holding a positioned marker, at three times its size:

| | border | marker sits at |
|---|---|---|
| no grid | 36 px | 132, 132 |
| grid, 0.6.9 | 36 px -- declined | 132, 132 |
| grid, now | **12 px** | **132, 132** |

The border is protected and the marker has not moved. Both, rather than one or the other.

Still declined, and both still right: an object placed **smaller** than it was drawn, where keeping
a border at its drawn size means magnifying the corner instead of protecting it; and a **cached**
object, which ignores a transform's scale when it is drawn back but not its shift.

## Text: this is what is actually wrong

Not fixed, but no longer guesswork. `DefineCSMTextSettings` is how a Flash author says how text
should be filled -- advanced antialiasing, whether to snap stems to the pixel grid, and a thickness
and sharpness to fill them with. Counted on the live build with
`cargo run -p swf --example text_settings_census`:

| what the game asks for | fields |
|---|---|
| advanced antialiasing | **627, every one** |
| with sub-pixel grid fitting | 500 |
| with pixel grid fitting | 105 |
| with a non-zero thickness or sharpness | 22 |

Aether parses all of it, stores it on the text object, and then draws the glyph outline plainly
regardless. Advanced antialiasing with grid fitting is what puts a glyph's stems on whole pixels
instead of smeared across two, which is the difference between text that reads as solid and text
that reads as thin and soft. That is the work, and it is a font rasterisation change rather than
anything in the layers looked at so far -- which is why the earlier suspects, cache resolution and
texture rounding and the antialiasing setting, all came back clean.

## From 0.6.8 and 0.6.9

**The movement stop guard runs for the first time.** It read the method's name out of the file to
decide whether a call was `walkTo` or `stopWalking`. AQW does not write those names: all 5,605
methods in the live build are nameless, while the staging build names every one of its 5,436. Every
check made against the staging file said the hook was fine while it had never once fired for a
player. It reads the trait name now, which stripping cannot remove.

## Downloads

`Aether-Setup-0.6.10-win-x64.exe` for the installer, `Aether-Portable-0.6.10-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.10-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
