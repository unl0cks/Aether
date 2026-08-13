Tooltips you can put where you want them, and options sorted into what they are for.

## Skill tooltips: above the pointer, or not at all

Two switches, both off unless you turn them on, under the new Gameplay tab.

**Show tooltips above the pointer.** AQW pins a skill's tooltip to the bottom-right corner of the
stage, which puts it over the bag icon, nowhere near the skill it describes, and on top of where
people click to move -- players have been stuck in place because a tooltip was in the way.
`ToolTipMC` already knows how to sit above a point; its cursor-following tooltips do exactly that.
This puts every tooltip there, using AQW's own offset so they match.

**Hide skill tooltips.** Stops a skill's tooltip appearing at all, which is the one that covers the
screen mid-fight. Tooltips for buffs and auras still appear, so you can still read what is on you.
The two are told apart by where AQW put them: a skill's is pinned to the corner, a buff's follows
the cursor, and that is decided on the frame it opens before anything here has moved it.

The account-safety warning is left alone by both. It is the one tooltip not attached to anything
hovered -- chat opens it, it closes itself after ten seconds, and it is meant to be read where it
was put.

Raised by PacketLoss and HardLuck.

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

`Aether-Setup-0.6.4-win-x64.exe` for the installer, `Aether-Portable-0.6.4-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.4-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
