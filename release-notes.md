A measurement, and a switch to settle the weapon glow with.

## The weapon glow, measured at last

Every previous attempt at this argued from screenshots. This one has numbers.

A test case draws one square with a glow on it, at sizes from 30 pixels across to 376, and the glow
is measured in pixels rather than looked at:

| square drawn at | square on screen | how far the glow reaches |
|---|---|---|
| 0.25x | 30 x 30 | 17 px |
| 0.5x | 70 x 70 | 19 px |
| 1x | 136 x 136 | 17 px |
| 2x | 256 x 256 | 17 px |
| 3x | 376 x 376 | 18 px |

The square grows twelve times over and the glow does not move. A glow covers a fixed number of
screen pixels no matter how large or small the thing wearing it is drawn, which is why it swamps a
small weapon and barely rims a large one, and why shrinking a character makes it worse.

That is not a mistake in Aether. It is what the renderer has always done, deliberately. Whether it
is what Flash did is the one thing that cannot be worked out from in here, and guessing at it has
now made things worse twice: measured against the object instead, most weapons lose their glow
almost entirely, while a few look better.

So both readings are in this build, and one run tells us which is right. Normally nothing changes.
Started this way, a glow is measured against the weapon wearing it instead of the screen:

    set AETHER_FILTER_SCALE=object

The same test case under that setting reaches 5, 9, 17, 35 and 53 pixels for the five sizes above,
in proportion to the weapon rather than fixed.

The earlier attempt at this measured the object with the wrong part of its transform, which holds
`scale x cos(angle)` and so falls to nothing at a quarter turn, taking the glow of every rotated
weapon with it. This one measures the length of the transform's basis vectors instead, so a weapon
turned ninety degrees renders a glow identical to one not turned at all. There is a test for it.

## Still to come

Frame rate is steadier in a crowded room than it was, but still not steady. Dragging the window
still costs frames while you are dragging.

## Downloads

`Aether-Setup-0.5.17-win-x64.exe` for the installer, `Aether-Portable-0.5.17-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.5.17-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
