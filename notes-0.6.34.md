The first full release since 0.6.32. It includes everything from the 0.6.33 test build.

## Fixed

**Low VRAM Mode never actually did anything.** The checkbox in the Aether options window saved its state and displayed it, and nothing read it. Only the `--low-vram` command line switch had any effect, which almost nobody uses. So anyone who turned the setting on carried on running the full texture budget with no way to tell.

That matters most on 0.6.32, which turned filter grouping on by default. Grouping raises the peak amount of video memory in use, and the smaller limits meant to offset that on a modest card were gated behind the same dead setting. If you have 4 GB of video memory or less, and 0.6.32 grew slower the longer you played until you restarted it, this is very likely why.

The setting now works from the options window. The command line still takes priority when you use it. The window will also tell you that a restart is needed, which it previously did not.

## Changed

**Cache texture recycling is now off by default.** 0.6.33 shipped it switched on so that it would get tested, on the theory that frame time spikes come from the graphics driver handing out fresh textures when a room fills up with players.

It has now been measured properly, by flipping it on and off every 60 seconds inside a single session so that both halves see the same rooms and the same crowds. Across 23 alternations it made no difference to frame time that could be told apart from noise, in either direction, at any scene size. It does recycle about two thirds of the textures it is asked for. That simply turns out not to be where the time goes.

The honest reading is that those texture allocations were a symptom of new content arriving rather than the cost of it. New content also means shape tessellation, image decoding and cache rebuilds, and recycling only removed the cheapest of those. A correlation that strong looked like a cause and was not.

Since it holds up to 192 MB of textures in reserve and buys nothing measurable, it now defaults to off. The switch is still in the Experimental tab if you want to try it, and it is worth trying on a slow drive or an unusual driver.

## New

**An "Experimental & Debugging" tab in the Aether options window.** Switches that previously needed an environment variable and a restart are now in one place, and they apply immediately:

* Blur filters together
* Recycle cache textures
* Share render passes between blends
* Move blends together first
* Retry building panel contents
* Log every complex blend

Anything you set from the command line is shown here but cannot be changed, so a test run cannot be quietly overridden by a saved preference.

**A "Check for updates" button**, in that same tab. It runs in the background, so a slow connection will not freeze the window.

**An "Include pre-releases" setting.** With it on, both the check at startup and the button will offer test builds as well as finished releases.

## Known issues

**The world map can start flickering, and this release does not fix it.**

What is known. It is triggered by clicking the map button repeatedly while the map is still opening. Once it has happened, every later click on the map button reopens the map flickering, for as long as the client stays open. Clicking the map button again closes it and stops the flickering until you next open it. Restarting the client clears it completely.

What is happening. The game's popup panels, meaning the map, bank, options, guild, house and the rest, are all frames on one shared timeline. Opening a panel asks that timeline to play forward to the panel you asked for, and the panel's own code is what stops it there. If it is not stopped, the timeline keeps going and walks through every other panel in turn, which is the flickering. One of the panels it walks through is the guild panel, whose code fails when it is reached this way and which, unlike the others, never stops the timeline. That is why it does not recover on its own.

What is not known yet is why the timeline fails to park the first time. A first attempt at a fix, based on the errors in the log, did not survive testing and has not been kept.

So this release improves the evidence instead of guessing again. When a panel's script fails, the log now records which panel it was, which frame it was on, and whether its timeline is still running, rather than the bare one line error it recorded before. Those three facts are enough to settle it. If you can make the map flicker, the log is worth sending:

```
%LocalAppData%\aether\log\ruffle.log
```

Reproducing it costs nothing and clears on restart, so it is safe to try.
