This release is about one thing: getting frames back. If 0.6.34 felt slower than everything before it, and especially if you ticked the new checkbox it shipped, this update is your frame rate back.

## Fixed

**The world map flicker.** Opening the map could leave it flashing between the game and an empty grey panel until you closed it, and once it started, every later open was broken until you restarted. The cause was a memory saving introduced a few versions ago: a file the game has already downloaded is kept and reused instead of being parsed again. The reused copy remembered most things, but not the piece of code that tells a freshly opened panel to stop and show itself, so a re-opened map played through its own frames forever. That piece is now carried over to reused copies too. The fix applies to everything the game re-loads the same way, not just the map.

**The "Retry building panel contents" setting is gone, and this is the honest part.** 0.6.34 added it as a possible repair for the map flicker. It did not fix the flicker, and it turned out that it could not have: by the time it ran, the game had already done everything the retry would try again. What it did do was cost an enormous amount of performance while switched on. Two separate parts of it each walked far too much of the game world far too often, and one of them quietly turned off an optimization for the whole screen on every frame. Measured on the same boss fight, the checkbox cut the frame rate roughly in half. A setting that cannot help and costs that much should not exist, so it has been removed rather than repaired. If you ticked it, updating is all you need to do; the old saved value is ignored.

## Faster

**Every internal script call in the game had been paying a small hidden tax, and fights paid it most.** Aether hooks a handful of the game's own functions to repair things like aura timers and effect positioning. The check that decided "is this call one of the hooked ones" was building several pieces of text for every single call the game makes, hundreds of thousands of times a second, and fights are where the game makes the most calls. That decision is now made without building anything, and only the handful of genuinely hooked calls pay the full cost. Busy fights with several players should hold their frame rate noticeably better than they have in any recent version.

**Two smaller costs of the same shape were also removed**: a timing measurement that ran for every script even though nothing was reading it, and a per-script check for two crafting overlays that did its expensive work before its cheap work.

## Changed

**Error messages in the log now say where the error happened.** A script failure used to log one anonymous line, so fifty identical lines could be one broken object or fifty different ones. Each line now names the object, its class, its frame, and whether it is still playing. When errors are suppressed to keep the log readable, the notice now says how many were suppressed, so a quiet log and a noisy one can no longer look the same.

## Notes

The frame rate collapse in 0.6.34 was hard to find for an unusual reason: the setting responsible defaults to off, so every comparison of the two versions side by side with fresh settings showed nothing. It only appeared for people who had switched the checkbox on, which included anyone who tried it against the map flicker on the release notes' own suggestion. The lesson has been taken: switches like that no longer get shipped as settings.
