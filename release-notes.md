The weapon glow, found.

## Weapon glows

A filter that draws outside the thing it is applied to has to be given room to do it, or it is cut
off at the edge of that room. Room is reserved for a blur, a glow, a drop shadow, a bevel and a
displacement map. It was never reserved for a **gradient** glow, which fell through to "reserve
nothing at all".

AQW's weapons wear a gradient glow. `UltimateGameClaymore.swf`, the Ultimate Flame Blade, carries
one 17 pixels wide over 3 passes, running from transparent red to opaque orange.

So the glow was cut off at the weapon's own outline, on all four sides, against a straight line.
Measured: at three sizes from 30 to 360 pixels across, the glow reached **zero pixels** past the
object. Not a small amount. None.

That is the rectangle. A glow is soft everywhere and nothing soft draws a straight edge, so the
rectangle was never the glow -- it was the edge of the space the glow was allowed to occupy. It got
worse as a character got smaller because the reach is measured in screen pixels while the outline
shrinks with the weapon, so a greater share of it fell outside.

Gradient glows and gradient bevels are now given the same room as the plain ones, plus the offset
they are drawn at.

## A division by zero in every blend that reads what is behind it

A blend that reads what is behind it -- Overlay, Multiply, Hardlight, Darken, Lighten, Difference,
Invert -- has to recover the colour underneath from a form that has already been multiplied by how
solid it is. Where nothing has been drawn behind, that is nothing divided by nothing.

The result of that is not a number, and unlike a real number it does not disappear when it is
multiplied by zero further along the same line. It survives to the screen, as a solid block the size
of whatever was being drawn.

Measured on a test case: a square blended this way over an empty background came out **solid black
across all 14,400 of its pixels**, and now comes out its own colour. Nothing else in the frame
moves.

The same recovery is guarded everywhere else in the renderer. These seven were the exception.

## The glow does not scale with the weapon, which is a separate question

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

## Frame rate around other players

The change in 0.5.16 that stopped holding a character as a finished image whenever anything inside it
blended is reverted. It cost frames in exactly the place they are worth most -- a crowded room is
mostly characters carrying blending weapons -- and it fixed nothing, because the fault it was aimed
at was the division by zero above, in a shader, rather than anything about caching.

## Still to come

Frame rate is steadier in a crowded room than it was, but still not steady. Dragging the window
still costs frames while you are dragging.

## Downloads

`Aether-Setup-0.5.19-win-x64.exe` for the installer, `Aether-Portable-0.5.19-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.5.19-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
