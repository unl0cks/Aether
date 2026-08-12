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

The old guess about the word "room" is gone. It existed to protect chat, and chat is no longer somewhere this runs.

## Lag when a control deck launches it

Reported by one of Laaiti's friends: slow when launched from a Fifine control deck, fine when the same `aether.exe` is double clicked. Same binary, same machine, same graphics card, so nothing about the rendering explains it.

A process inherits two things from whatever started it, and a control deck is a background helper that has both. Priority class is handed straight down. Power throttling is the one that hurts: Windows 11 puts a process it considers background into a low power mode that parks it on efficiency cores and holds the clocks down, and a child of a background process inherits that. For something drawing frames it reads exactly like a slow computer.

Aether now switches that throttling off for itself at startup and lifts its priority back to normal if it was handed something lower. It does not raise itself above normal in either case.

If this was the cause, the deck and the desktop shortcut should now behave the same. If it is still slower from the deck, it is something else and worth saying so.

## Downloads

`Aether-Setup-0.5.14-win-x64.exe` for the installer, `Aether-Portable-0.5.14-win-x64.zip` if you would rather not install anything.

There may also be `Aether-Launcher-0.5.14-win-x64.exe`. It is the same installer at a few megabytes instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It needs a connection; the other two do not.
