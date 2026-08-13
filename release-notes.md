A pre-release. Panel corners are decided per axis now, which puts them back on the tooltips that
lost them.

## Corners came back off because of a rule I added

A border is kept at its drawn size by dividing it by the object's scale. Below one that division
makes the band *larger* than it was drawn, so the piece covering the corner magnifies whatever it
reaches instead of protecting it. 0.6.9 dealt with that by refusing to slice anything being drawn
smaller than it was made.

That refusal was for the whole object, and it should never have been. A tooltip is sized to fit its
text, so it routinely **grows along one axis while shrinking along the other** -- and refusing both
is what put the stretched corners back on the aura tooltips and the skill tooltips that had already
been fixed once.

Each axis decides for itself now. A growing axis has its borders kept; a shrinking one is passed
straight through and draws exactly as it always did. Measured on a bordered box, with the horizontal
grown three times and the vertical shrunk to 0.6, which is the shape a tooltip actually takes:

| | left border | top border |
|---|---|---|
| no grid | 36 px | 7 px |
| grid, 0.6.9 to 0.6.11 | 36 px -- refused outright | 7 px |
| grid, now | **12 px** | 7 px |

Grown on both axes still gives 12 and 12, and shrunk on both is still pixel for pixel what it is
with no grid at all, so the magnified corner that rule was written for stays fixed.

## Not fixed in this build

Three things reported alongside the corners are **not** in here, and I would rather say so than
imply otherwise:

- The skill icon inside its grey button, the drop-accept checkmark, and the text inside an aura
  tooltip all sit left of centre. AQW centres each of these by measuring another object -- the
  pattern is `x = container.width / 2 - content.width / 2` -- so this is about a reported width
  rather than about drawing. Bounds have been checked against Flash on three counts so far and match
  on all three: they exclude filters, they include invisible children, and shape bounds include
  stroke widths.
- Text is thin and soft. The cause is known: **627 text fields in the live build, every one of them
  asking for advanced antialiasing**, 500 with sub-pixel grid fitting and 105 with pixel grid
  fitting, all of it parsed, stored, and then ignored at draw time.
- Frame rate decaying over a session. Measured at 127,487 offscreen texture allocations totalling
  1.37 TB in one two-minute trace, with every allocation past the forty-second mark evicted for
  budget as soon as it is made.

## Downloads

`Aether-Setup-0.6.12-win-x64.exe` for the installer, `Aether-Portable-0.6.12-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.12-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
