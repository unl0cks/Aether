Two real rendering faults fixed, and the frame rate given back.

## A gradient glow was given no room to draw

A filter that draws outside the thing it is applied to has to be given room to do it, or it is cut
off at the edge of that room. Room was reserved for a blur, a glow, a drop shadow, a bevel and a
displacement map. It was never reserved for a **gradient** glow, which fell through to reserving
nothing at all.

AQW's weapons wear a gradient glow. `UltimateGameClaymore.swf`, the Ultimate Flame Blade, carries
one 17 pixels wide over 3 passes, running from transparent red to opaque orange.

Measured with that exact filter at three sizes, from 30 to 360 pixels across, the glow reached
**zero pixels** past the object. Not a small amount. None. It now reaches 10 to 15.

Gradient glows and gradient bevels are now given the same room as the plain ones, plus the offset
they are drawn at.

Weapons cost slightly more to draw than they did, because a glow that is allowed to exist takes up
room that was previously not allocated at all.

## A division by zero in every blend that reads what is behind it

Overlay, Multiply, Hardlight, Darken, Lighten, Difference and Invert each recover the colour
underneath from a form that has already been multiplied by how solid it is. Where nothing has been
drawn behind, that is nothing divided by nothing.

The result is not a number, and unlike a real number it does not disappear when it is multiplied by
zero further along the same line. It survives to the screen as a solid block the size of whatever
was being drawn.

Measured: a square blended this way over an empty background came out solid black across all 14,400
of its pixels, and now comes out its own colour. Nothing else in the frame moves. The same recovery
is guarded everywhere else in the renderer; these seven were the exception.

## Frame rate around other players

The change in 0.5.16 that stopped holding a character as a finished image whenever anything inside
it blended is reverted. It cost frames in exactly the place they are worth most -- a crowded room is
mostly characters carrying blending weapons -- and it fixed nothing, because the fault it was aimed
at was the division by zero above, in a shader, rather than anything about caching.

An experimental switch for how filters are scaled has been removed. It was there to compare two
readings of one measurement, the comparison was made, and neither reading changed anything.

## Still to come

The weapon glow is still wrong in the game. Both faults above are real and both are fixed, and
neither was the whole of it: the weapon renders correctly here at every size, angle and nesting that
can be built outside the game, so what remains is in what a character wraps around it.

Frame rate is steadier in a crowded room than it was, but still not steady. Dragging the window
still costs frames while you are dragging.

To find the rest of the glow, this build can report what a filtered object is wrapped in. Started
with `AETHER_GLOW_ANCESTRY=1`, it writes one line per filtered object naming every container between
it and the stage, with each one's scale, blend mode, mask and cache state, followed by the room
reserved for the filter. A glow that is cut off will say it grew by nothing.

## Downloads

`Aether-Setup-0.5.20-win-x64.exe` for the installer, `Aether-Portable-0.5.20-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.5.20-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
