A pre-release. Panels draw as they did before, and the scaling-grid work is behind a switch.

## Scaling grids are opt-in now

Drawing a panel in nine pieces so its borders keep their size is right in principle, and measurably
does what it is for: a 12 pixel border on a box drawn at three times its size stayed 12 pixels
instead of becoming 36.

It has also moved things that were correct before. Buttons and icons sat up and to the left, tooltip
corners came out square, and a seam could run through a panel. Two attempts at confining it have
each fixed one report and left another, and nothing in here can tell a panel that now draws right
from one that now draws wrong without it being looked at.

So it is off by default, and everything draws as it did in 0.6.4. It is under Advanced, as **Keep
panel borders unstretched**, for anyone who wants to compare the two.

## Buff tooltips are a switch of their own

**Show buff and aura tooltips**, in the Gameplay tab, now decides whether that tooltip appears at
all rather than only whether it is kept on screen. On -- which is the default -- it is kept beside
the pointer whatever else is set, so it survives skill tooltips being hidden. Off, it does not
appear. Skill tooltips are unaffected either way.

**Tooltips above the pointer** is gone from Advanced. It lives in Gameplay and was in both.

## Rounded corners stopped becoming ellipses

A scaling grid is how a panel says which parts of it may stretch. The corners and borders keep the
size they were drawn at, and only the middle grows. It is what stops a rounded corner turning into
an ellipse when a box is made bigger.

Aether read the grid out of the file and answered it to the game's own code, but never used it when
drawing, so a panel resized by setting its width -- which is how AQW sizes every tooltip, window and
message box -- had its corners stretched along with its middle. The bigger the panel, the rounder
the corners. That is why a short tooltip looked right and a long one bulged.

Measured on a bordered box with a 12 pixel border, drawn at three times its size:

| | box | border |
|---|---|---|
| drawn at 1x | 100 x 100 | 12 px |
| 3x, as before | 300 x 300 | 36 px |
| 3x, now | 300 x 300 | **12 px** |

This is the first build with it, and it touches every panel in the game rather than only the ones
that looked wrong, which is why this is a pre-release. If something that used to draw correctly now
does not, that is this.

## The traces work in a normal build

`--input-trace`, `--timeline-trace` and `--frame-construction-retry` all reported that the binary
was built without the feature they needed, in every binary anyone has. They were gated on `metrics`,
which also turns on GPU counters and a per-frame resource census and is far too expensive to ship,
so they were unreachable outside a build made specially.

They are diagnostics rather than measurements, and are now gated on that instead. The trace they
write is what says *why* a walk stopped early: every declined stop is attributed to the first gate
that turned it down, out of ten, which is the difference between "the guard ran and disagreed" and
"the guard was never reached at all".

## Tooltips that were hidden come back

Turning "hide skill tooltips" off left every skill tooltip invisible for good. Whether a tooltip was
open was read from the same flag that hiding one clears, so once hidden it could not be found again,
and so could never be put back. AQW's own contents now decide whether a tooltip is open; the flag is
Aether's alone, and it is remembered when Aether is the one that cleared it.

**Always show buff and aura tooltips** is a third switch, so the tooltip that names what is on you
survives skill tooltips being suppressed, and is kept beside the pointer and on screen. AQW has its
own switch for these under Class Actives/Auras UI; if that one is off, nothing here can bring them
back.

## The options window is sorted by what things are for

Three tabs now. **Gameplay** for what the game does: number grouping and the tooltip switches.
**Visuals & Performance** for how it looks and how fast it runs, which is what the Game tab held.
**Advanced** is unchanged.

## A gradient glow is a ramp, and was being drawn as one flat colour

A gradient glow changes colour with distance from the object -- that is the whole reason it is a
different filter from a glow. Aether kept a single stop out of the ramp and drew the whole glow in
it.

The Ultimate Flame Blade authors two stops: transparent **red** at the outside running to opaque
**orange** at the blade. Measured across its own glow, from the outer edge inward, as the gap
between the green and blue channels -- zero is pure red, wide is orange:

| into the glow | before | now |
|---|---|---|
| outer edge | 4 | **0, pure red** |
| 12 px | 18 | 3 |
| 24 px | 38 | **14, warming to orange** |

So the outer bloom is red and ramps to orange at the blade, which is what the artwork says and what
other clients show. The whole ramp is carried now, up to fifteen stops, and a plain glow is simply
the case with no ramp at all -- it renders exactly as it did.

A glow still covers the same number of screen pixels however large or small the thing wearing it is
drawn. Making it shrink with the object was tried in between and was wrong: on an avatar drawn at a
third of its authored size a 17 pixel blur becomes six, and the glow all but disappears. What looked
like a glow fading out on a small character was this flat colour, not its size.

## Updates that could not be downloaded

Two players were offered an update and then told it could not be downloaded. GitHub serves a release
asset from two different hosts: the link in the release page redirects to `objects.githubusercontent
.com`, while the API serves the same bytes from `api.github.com`. The update check had already
talked to the API successfully -- that is how they were offered the update at all -- and only the
redirect target could not be reached.

Both routes are tried now, each three times, and the message says what actually went wrong instead
of only that something did. Connecting has its own short timeout so an unreachable host fails
quickly, while the download itself is allowed ten minutes for a slow line.

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

Frame rate is steadier in a crowded room than it was, but still not steady. Dragging the window
still costs frames while you are dragging.

`AETHER_GLOW_ANCESTRY=1` still makes each filtered object report what it is wrapped in: every
container between it and the stage, with each one's scale, blend mode, mask and cache state, then
the room reserved for its filter. It is what found two of the four faults above, and it stays.

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

`Aether-Setup-0.6.7-win-x64.exe` for the installer, `Aether-Portable-0.6.7-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.7-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
