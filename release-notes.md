The glow no longer fades out on a small character, and a mode for cards short of memory.

## Weapon glows keep their proportions at any size

The clipping is fixed. What was left was a glow that thinned out as a character got smaller, which
is the same fault seen from the other side.

A blur was measured in screen pixels rather than in the pixels of the thing wearing it. The same
kernel that keeps a full-size weapon's glow bright spreads a quarter-size one's thin, so the glow
did not shrink with the weapon -- it diluted.

Measured on the real Ultimate Flame Blade, drawn at half and quarter size, as a fraction of its
full-size width:

| drawn at | before | now | should be |
|---|---|---|---|
| 0.5x | 0.534 | 0.511 | 0.50 |
| 0.25x | 0.301 | 0.262 | 0.25 |

Filters now scale by the whole transform down to the object, taken as the length of the transform's
basis vectors rather than as two of its four numbers, so a weapon turned a quarter turn keeps the
size its matrix actually carries.

If a map background looks softer than it did, that is this change and worth reporting.


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

## Low VRAM mode

A new switch under Advanced options, off by default. It keeps a much smaller budget of textures for
reuse.

On a card with memory to spare this **costs** frames: the tuning runs that set the current budget
measured 62.2% texture reuse at the small size against 99.8% at the full one, so roughly four times
as many fresh allocations per frame.

On a card without, it should gain them. Two pools at the full budget is a gigabyte of retention
before a single live render target, and a 2 GB card holding that spills into system memory instead
-- paid on every access, every frame, rather than once on a miss. That spilling is also what the
coloured pixel noise and diagonal streaks reported on a GT 1030 look like.

Try it if you see either.

## Downloads

`Aether-Setup-0.5.21-win-x64.exe` for the installer, `Aether-Portable-0.5.21-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.5.21-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
