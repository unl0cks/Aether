A small release with one privacy fix that matters, one chat annoyance gone, and groundwork for the frame rate work.

## Your logs no longer name a stranger

Rust compiles the source folder paths into the binary, which meant every log file and crash report quoted the Windows account name of whoever built the release. If you have ever sent a log and wondered who the name in it belonged to, that was the person who built your copy of Aether.

The stripping for this was written for 0.5.10 but only applied to command line builds, not to the installer, so 0.5.10 shipped with it anyway. It is in the installer build now, and the build refuses to finish if the name survives into the binary, so it cannot quietly come back.

## Numbers in chat

Typing `battleon 99222` in chat sent it as `battleon 99,222`. The digit grouping that makes gold and boss health readable was treating a room number as a quantity.

It no longer touches chat at all. Grouping now applies only to fields that hold a single value written by the game, which is what gold, experience, boss health, damage numbers and quest counters are. Chat is a log, and a log is left exactly as typed.

That also explains why the number looked fine while you were typing it and gained a comma once you pressed send: the box you type in was already excluded, and the log it landed in was not.

Thanks to Necro for spotting it.

## Version numbers

Every release so far has called itself something like `0.5.10-local` in logs and crash reports. That was a build setting nobody had set, so published builds carried a developer's suffix. Released builds now say `0.5.11-release`.

## Frame rate groundwork

No speed change in this build. A diagnostic build can now report which art in the game is issuing the expensive drawing work, aggregated per symbol rather than per object, so fifteen players wearing the same armour show up as one entry rather than fifteen.

The first results are worth saying out loud, because they were not what anyone expected:

- The map is the largest single cost, not player avatars. One Battleon overlay layer redraws 4.0 megapixels every time it is composited, on a stage that is only 960 by 550, because a cached map sizes its work to the whole map rather than to what is on your screen.
- 89% of all blended layers wrap a single shape. Each one currently allocates its own offscreen image, draws one shape into it, copies the background back out, and composites, for one shape.

Both of those are fixable and both are next.

## Downloads

`Aether-Setup-0.5.11-win-x64.exe` for the installer, `Aether-Portable-0.5.11-win-x64.zip` if you would rather not install anything.

There may also be `Aether-Launcher-0.5.11-win-x64.exe`. It is the same installer at a few megabytes instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It needs a connection; the other two do not.
