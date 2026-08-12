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

Every release so far has called itself something like `0.5.10-local` in logs and crash reports. That was a build setting nobody had set, so published builds carried a developer's suffix. Released builds now say `0.5.13-release`.

## Anti-aliasing quality no longer wrecks the frame rate

Changing quality down to Low or Medium and back up to High or Best dropped the client to a few frames a second, and stayed that way until you restarted it.

Every offscreen image the renderer keeps to reuse is tied to the anti-aliasing level it was made at, so changing that level made the whole cache unusable. It was kept anyway, while a full second set was built alongside it. On a 10 GB card that pushed graphics memory to 14.2 GB, and past that point the driver starts shuffling images over the bus, which is the same frame doing the same work ten times slower.

That also explains why only one direction was slow. Going down to Low is fine, because the new set is the small one. Going back up is what crossed the line. The unusable images are now dropped when the level changes.

Thanks to Laaiti for finding the exact steps to reproduce it; the fact that only one direction lagged is what identified the cause.

## The launcher records what it installed

A 0.5.11 launcher that downloaded and installed 0.5.12 wrote 0.5.11 into Windows as the installed version. Add/Remove Programs showed the wrong number, and the update check read it back and offered an update you already had. It now records the version it actually installed, so one launcher keeps working rather than needing a fresh one each time.

## Downloads

`Aether-Setup-0.5.13-win-x64.exe` for the installer, `Aether-Portable-0.5.13-win-x64.zip` if you would rather not install anything.

There may also be `Aether-Launcher-0.5.13-win-x64.exe`. It is the same installer at a few megabytes instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It needs a connection; the other two do not.
