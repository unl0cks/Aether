A pre-release. The thin text was AQW turning its own antialiasing off, and it no longer can.

## Why the text was thin

AQW manages its own quality. `World.as` samples the frame rate and, on an average below twelve
frames a second, steps `stage.quality` down a level:

    internal var arrQuality:Array = new Array("LOW","MEDIUM","HIGH");
    ...
    if (avgFps <  12 && idx > 0) stage.quality = arrQuality[idx - 1];
    if (avgFps >= 12 && idx < 2) stage.quality = arrQuality[idx + 1];

`HIGH` is four times multisampling. `MEDIUM` is two. **`LOW` is none at all.** So a dip below twelve
leaves every piece of vector art in the game -- which is all of the text -- drawn with no
antialiasing whatsoever. It climbs back one level per five samples of twenty-four frames, so a
moment of slowness costs hundreds of frames of soft, thin text well after the slowness has gone.

That is reasonable of AQW, which cannot know what it is running on. It is not reasonable here: the
quality setting exists so the player can say what their card should be asked for, and a transient
dip should not overrule them.

The stage is now held at the quality that was chosen. A movie asking for less is declined; asking
for more is allowed. The setting is unchanged and still under Visuals & Performance, so `--quality
low` still gets low.

This is also why the text looked worse at some moments than others, and why it could not be found
in the font handling or the cache or the renderer's own antialiasing: none of those were wrong. The
game had turned the antialiasing off.

## What this does not fix

The frame rate dips that trigger it. Measured on a fresh nine-minute capture: the offscreen texture
pool sits at its 512 MiB ceiling and evicts essentially every texture it is handed -- 1,375
allocated against 1,341 evicted in one interval -- while resident texture memory peaks at **9.5 GB**
against a p95 frame time of 161 ms. On a 10 GB card that costs frames. On the 2 GB card that
reported coloured noise and a yellow band across the screen, it runs the card out of memory, which
is what that looks like and why relogging cleared it. One fault, two symptoms, and still the
outstanding one.

The first hundred seconds of that capture are also slow -- 45 to 53 ms a frame -- with no texture
churn at all and 99% pool reuse, so that is something else again.

## Also still open

The drop-accept checkmark sits left of centre. AQW centres it by measuring another object, so it is
a reported width rather than anything about drawing.

`playerAuras.countDownAct` throws `Error #1009` continuously during combat, alongside definition
lookup misses for the aura clips it is trying to advance.

## Downloads

`Aether-Setup-0.6.14-win-x64.exe` for the installer, `Aether-Portable-0.6.14-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.14-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
