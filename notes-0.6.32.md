Pre-release. Two memory fixes and a rendering change, all measured against live sessions rather than estimated.

## Memory

**`flash.utils.Dictionary` now honours `weakKeys`.** It previously accepted the argument and then held its keys strongly anyway, which is the opposite of what it asks for and had no upper bound: AQW keeps static weak dictionaries keyed on UI components, so every component ever registered stayed reachable, and through it the class, the application domain, and the whole movie it came from.

Measured over a 3 hour 46 minute session: 2,956,766 dead keys swept, a figure that was structurally zero before. Resident memory grew 19.3 MB a minute against roughly 52 GB across five hours previously, and the collector's own numbers now rise and fall instead of only rising.

What still grows is a separate problem and is being worked on: loaded assets are never released, so vector shapes and clip definitions accumulate at about one copy of each distinct file.

**Weak dictionaries keyed by number or name no longer pay for a sweep they cannot use.** `weakKeys` only weakens object keys, and four of AQW's busiest dictionaries are keyed by user id, name or item id. Those are enumerated many times a frame and were each walking their whole table to remove nothing.

## Rendering

**Objects that share a blur are now blurred together.** Every `cacheAsBitmap` object carrying a glow used to run its own blur: two intermediate targets and several render passes each, every frame, even when a dozen objects on screen were running the identical kernel. They are now packed into one padded texture and blurred once, and each object then reads its own part of the result.

Measured with an A/B over two sessions matched on scene complexity, the cost of a filtered cache entry fell from 0.085 ms to 0.025 ms, a 71% reduction, while unfiltered work stayed flat. Groups average about four objects, and the padding costs about 1.3 times the area it covers.

The effect grows with the number of glowing objects on screen, so crowded maps gain most.

Set `AETHER_FILTER_ATLAS=0` to composite each filter separately as before. On systems using Low VRAM mode the groups are kept smaller, since holding several targets together costs peak video memory.

## Fixes

**Avatars and effects no longer twitch when a cached object carries a filter.** The adaptive avatar cache could abandon a cache that was still needed, and dropping it dropped the filter and the pixel snapping with it.

## Notes

Frame rate figures from any single session are dominated by how busy the map is. The numbers above come from comparing sessions matched on how much was drawn, which is the only way these changes can be told apart from the room simply being quieter.
