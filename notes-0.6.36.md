This release is mostly about speed in the places the game is busiest: crowded towns, and fights where somebody casts an effect on the whole party. A handful of visible annoyances are fixed along the way, and the FPS counter finally has proper controls.

## Faster

**Crowded towns.** Battleon was running at roughly 30 frames per second and now runs at about 56, same machine, same spot. The cause sounds too small to matter. The game re-states the position and colour of its scenery every single frame, usually with values that have not changed at all, and Aether treated every one of those as a change: it threw away the finished picture of the entire layer and drew it again from scratch. A write that does not actually change anything is now recognised as such, and the finished picture survives. Everything the game re-states without changing benefits from this, not only the towns where it was measured.

**Effects that put a glow on everyone.** Skills that apply a large visual effect to your whole party, of which Divine Intervention is the heaviest in the game, were dropping the frame rate hard. Three separate costs were stacked on top of each other and all three are now addressed.

The wings and effects like them are not one big picture; they are dozens of small glowing layers per player, and every one of those layers was being drawn in a step of its own. Related layers now draw together. Separately, every layer that mixes with what is behind it was also being prepared on its own, and those are now prepared together too: in a busy fight this turned more than four hundred separate preparation steps into five. Finally, the picture behind each mixing layer was being rebuilt in full before every single one of them, even when nothing behind it had changed since the last time. That rebuild is now skipped when the area really is untouched.

Party-wide effects should be considerably lighter than they were. The heaviest ones can still dip on a busy screen, and there is more to come for them.

**Memory that never came back.** Several kinds of graphics memory could only grow. Text was the worst: the game loads a font with almost every piece of equipment, hundreds over a session, and the prepared letters from all of them were kept forever, long after nothing on screen used them. They are now released when idle and rebuilt on demand. Cached pictures belonging to objects the game keeps alive in its own bookkeeping, which the previous cleanup could not reach at all, are now reachable as well.

There is also a ceiling on graphics memory now. Past it, new cached pictures are declined and the object is simply drawn directly, which looks identical. This matters because of what it prevents: one player's session climbed to 3.3 GB of graphics memory on a 4 GB card within three minutes, at which point the card starts spilling into system memory and everything stops. A single misbehaving effect can do that, and the ceiling stops it.

## Fixed

**Black lines across item drops, confirmation windows and tooltips.** Panels in the game are drawn as nine pieces so their borders keep their thickness when the panel is resized. Where two of those pieces met, they were being overlapped very slightly, on the reasoning that drawing the same artwork twice in a hairline costs nothing. That is true when the artwork is solid and false when it is see-through: the overlap was painted twice and came out darker, which is the line. Tooltips only showed it while fading out, which is exactly when their pixels become see-through, and that was the clue that identified it. The pieces now meet exactly instead of overlapping.

**The world map kept moving after you let go of the mouse.** If a map drag ended with the pointer over the top of the window, the release never reached the game, and the map stayed stuck to the cursor until the next click. The menu bar was claiming the release for itself simply because the pointer was over it. A drag that begins in the game now keeps the pointer until it ends, wherever it ends.

## Added

**The FPS counter can be given a key, a position and a size.** The keybinds menu offers a proper key binding now: click the button, press whatever key or combination you want, or clear it. The counter can sit in the top left, top centre or top right, at three sizes, and it stays above the game's own panels instead of being covered by whatever you open next.

## Notes

**Integrated graphics.** Players on integrated graphics, where the graphics chip borrows system memory rather than having its own, have been running out of memory and losing the display driver. The new memory ceiling detects those machines and applies a much tighter limit automatically, rather than waiting to be asked for it through a setting named after video memory those machines do not have. The crash that follows a driver loss is not ours to fix and is still there, but there is now one more thing standing between it and you.

**On measurement.** Every rendering change in this release was checked by rendering the same test movies before and after and comparing the results pixel for pixel. All of them come out identical, which is the point: this release is meant to be the same game, faster.
