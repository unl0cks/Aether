Aether has been loading the wrong AQW build this whole time. That is the headline; the rest follows from finding it.

## You were playing an old version of the game

AQW's web page loads `Loader3.swf`. Aether loaded `Loader_Spider.swf`. They are the same file apart from one line, and that line is which game build to fetch.

`Loader3.swf` asks AQW which build is current and loads the answer, which today is `Game3098r24.swf` and reports itself as version 4.361. `Loader_Spider.swf` is Artix's staging loader and asks for nothing; it hardcodes `spider.swf`, a frozen build that reports 4.26. That is the number Necro spotted in the corner of the options panel, and it was right: this client was several releases behind everyone else.

Aether now loads the same loader the website does, so it gets whatever build AQW is currently serving.

One consequence worth stating plainly. Every compatibility repair in Aether was written against `spider.swf` and gated on recognising that file by name, so switching builds would have silently switched all of them off: the aura fixes, the avatar cache, the movement guard, the settings panel repair. Those gates now ask one shared question, which handles a build name that changes every release, and each one has a test pinning it. But the game itself is a build nobody has run Aether against yet, so if something behaves oddly that did not before, that is the first place to look.

## Numbers, everywhere except where somebody typed them

0.5.13 stopped chat being rewritten by only grouping numbers in single-line fields. That was wrong and it took the quest panel with it, because a reward list wraps like any other paragraph. A quest panel and a line of chat are the same kind of field: read-only, multiline, word-wrapped, HTML. Nothing about a field says whether it holds a quantity or a sentence.

The fix after that named the fields to group instead, which worked but was never going to finish. Gold and health were obvious, then quest rewards, then the experience bar, then item stacks, then the reputation panel. Each one had to be noticed by somebody before it could be fixed.

So it is the other way round now. Every field the game writes gets separators, and grouping is refused in the places a player writes: the chat log, in both the current chat interface and the older one, and the speech balloon over a character's head. That list is short and it is finished, which the other one was not.

Which means these all read properly now, including the three that were still missing: the experience bar, item counts in the inventory, and the reputation standings list. Along with gold, health, mana, soul points, class rank, AC, quest rewards and requirements, and the damage, healing and critical numbers that float over a character.

Two things are still never touched. An editable field, because that is one the game reads back, and a separator in one would send `1,250,000` where `1250000` was meant. And a number welded to a name, which is what keeps `citadelruins-99922` a room rather than a count.

A third thing changed with them. Separators make a number wider, and a field drawn to fit six digits does not grow to fit eight; Flash cuts it off at the edge instead. The reputation panel showed this as `100,00`. A column that is too narrow for its separators now takes the room it needs from the left, which leaves the right edge where it was drawn and grows the number into the gap beside it. If even that is not enough, the number is written as the game sent it.

## Dragging the window no longer wrecks the client

Moving or resizing the window made everything slow, and staying at the new size did not undo it. Long enough at it and the client ran out of graphics memory and closed.

Windows reports a resize once per frame for the whole time a window is being dragged, and reports one for a plain move as well, where nothing has changed at all. Every one of those emptied both texture caches and rebuilt every render target. So a two second drag threw away and rebuilt the entire cache a hundred times, and a graphics driver cannot reclaim a texture until the card has finished with the frame that used it. Asked faster than the card retires them, the ones waiting to be freed are the memory. A 10 GB card reached 44.2 GB that way while the renderer had only asked for 14.2 GB, and died on the next large allocation, which is why maximising the window was often the moment it went.

Three things changed. A resize is now applied once per drawn frame rather than once per report, so a drag costs one rebuild a frame instead of a hundred a second. A report for the size already in use is dropped entirely, which is every report a window move produces. And a real resize now keeps both caches, because nothing in them depends on how big the window is: a texture is keyed by its own size, and the ones that no longer suit the new viewport expire on their own a couple of seconds later.

Thanks to Laaiti for finding it, and for going back and breaking it again twice after the first fix. The client had been crashing without a pattern, and the pattern was the window.

The old guess about the word "room" is gone. It existed to protect chat, and chat is no longer somewhere this runs.

## Lag when a control deck launches it

Reported by one of Laaiti's friends: slow when launched from a Fifine control deck, fine when the same `aether.exe` is double clicked. Same binary, same machine, same graphics card, so nothing about the rendering explains it.

A process inherits two things from whatever started it, and a control deck is a background helper that has both. Priority class is handed straight down. Power throttling is the one that hurts: Windows 11 puts a process it considers background into a low power mode that parks it on efficiency cores and holds the clocks down, and a child of a background process inherits that. For something drawing frames it reads exactly like a slow computer.

Aether now switches that throttling off for itself at startup and lifts its priority back to normal if it was handed something lower. It does not raise itself above normal in either case.

If this was the cause, the deck and the desktop shortcut should now behave the same. If it is still slower from the deck, it is something else and worth saying so.

## Downloads

`Aether-Setup-0.5.14-win-x64.exe` for the installer, `Aether-Portable-0.5.14-win-x64.zip` if you would rather not install anything.

There may also be `Aether-Launcher-0.5.14-win-x64.exe`. It is the same installer at a few megabytes instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It needs a connection; the other two do not.
