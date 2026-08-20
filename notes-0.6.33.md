Pre-release. One fix that matters if you are on a card with limited video memory, plus new controls for testing.

## Fixed

**Low VRAM Mode never actually did anything.** The checkbox in the Aether options window saved its state and displayed it, and nothing read it. Only the `--low-vram` command line switch had any effect, which almost nobody uses. So anyone who turned the setting on carried on running the full texture budget with no way to tell.

That matters most in 0.6.32, which turned filter grouping on by default. Grouping raises the peak amount of video memory in use, and the smaller limits meant to offset that on a modest card were gated behind the same dead setting. If you have 4 GB of video memory or less and 0.6.32 grew slower the longer you played until you restarted, this is very likely why.

The setting now works from the options window. The command line still takes priority when you use it. The window will also tell you that a restart is needed, which it previously did not.

## New

**An "Experimental & Debugging" tab in the Aether options window.** Switches that previously needed an environment variable and a restart are now in one place and apply immediately:

* Blur filters together
* Recycle cache textures
* Share render passes between blends
* Move blends together first
* Log every complex blend

Anything you set from the command line is shown here but cannot be changed, so a test run cannot be quietly overridden by a saved preference.

**A "Check for updates" button**, in that same tab. It runs in the background, so a slow connection will not freeze the window.

**An "Include pre-releases" setting.** With it on, both the check at startup and the button will offer test builds like this one as well as finished releases.

## Changed

**Cache textures are now reused rather than reallocated.** Every object the game keeps as a bitmap used to ask the graphics driver for a fresh texture, and a room filling up with players asks for dozens at once, which is where frame time spikes come from. They now come from a pool.

Being honest about this one: it does what it is supposed to mechanically, reusing about three quarters of them, but it has not yet been shown to make the game measurably smoother. It is on by default in this test build so that it gets exercised. Turn off "Recycle cache textures" in the new tab if you would rather not have it, and please say so if anything cached ever draws the wrong thing.

## Notes

Frame rate figures from a single session are dominated by how busy the map is, so comparing two sessions in different places says very little. The controls above are runtime switches for exactly that reason: a change and its absence can now be compared inside one session.
