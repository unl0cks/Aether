A small release with one privacy fix that matters, one chat annoyance gone, and groundwork for the frame rate work.

## Your logs no longer name a stranger

Rust compiles the source folder paths into the binary, which meant every log file and crash report quoted the Windows account name of whoever built the release. If you have ever sent a log and wondered who the name in it belonged to, that was the person who built your copy of Aether.

The stripping for this was written for 0.5.10 but only applied to command line builds, not to the installer, so 0.5.10 shipped with it anyway. It is in the installer build now, and the build refuses to finish if the name survives into the binary, so it cannot quietly come back.

## Room numbers in chat

Typing `join room 9922` turned into `join room 9,922`. The digit grouping that makes gold and boss health readable was treating a room number as a quantity. It already left `citadelruins-9922` alone, because the hyphen makes it obviously part of a name, but a number with spaces around it looked like a count.

Now the word in front decides. A number introduced by room, rooms, join or goto is a name and is left exactly as typed. It only covers the number it introduces, so `room 9922 has 1250000 gold` still separates the gold.

Thanks to Necro for spotting it.

## Frame rate groundwork

No speed change in this build. A diagnostic build can now report which art in the game is issuing the expensive drawing work, aggregated per symbol rather than per object, so fifteen players wearing the same armour show up as one entry rather than fifteen.

The first results are worth saying out loud, because they were not what anyone expected:

- The map is the largest single cost, not player avatars. One Battleon overlay layer redraws 4.0 megapixels every time it is composited, on a stage that is only 960 by 550, because a cached map sizes its work to the whole map rather than to what is on your screen.
- 89% of all blended layers wrap a single shape. Each one currently allocates its own offscreen image, draws one shape into it, copies the background back out, and composites, for one shape.

Both of those are fixable and both are next.

## Downloads

`Aether-Setup-0.5.11-win-x64.exe` for the installer, `Aether-Portable-0.5.11-win-x64.zip` if you would rather not install anything.
