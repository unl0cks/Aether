Aether has been loading the wrong AQW build this whole time. That is the headline; the rest follows from finding it.

## You were playing an old version of the game

AQW's web page loads `Loader3.swf`. Aether loaded `Loader_Spider.swf`. They are the same file apart from one line, and that line is which game build to fetch.

`Loader3.swf` asks AQW which build is current and loads the answer, which today is `Game3098r24.swf` and reports itself as version 4.361. `Loader_Spider.swf` is Artix's staging loader and asks for nothing; it hardcodes `spider.swf`, a frozen build that reports 4.26. That is the number Necro spotted in the corner of the options panel, and it was right: this client was several releases behind everyone else.

Aether now loads the same loader the website does, so it gets whatever build AQW is currently serving.

One consequence worth stating plainly. Every compatibility repair in Aether was written against `spider.swf` and gated on recognising that file by name, so switching builds would have silently switched all of them off: the aura fixes, the avatar cache, the movement guard, the settings panel repair. Those gates now ask one shared question, which handles a build name that changes every release, and each one has a test pinning it. But the game itself is a build nobody has run Aether against yet, so if something behaves oddly that did not before, that is the first place to look.

## Numbers in chat, properly this time

The last release stopped chat being rewritten by only grouping numbers in single-line fields. That was wrong, and it took the quest panel with it. Necro's quest rewards stopped showing separators, and anything else that wraps would have too.

The reason it was wrong: a quest reward panel and a line of chat are the same kind of field. Both read-only, both multiline, both word-wrapped, both HTML. Nothing about the field says which is a quantity and which is a sentence, so no test of that sort could ever have separated them.

Grouping is now granted by name, to the fields AQW writes a number into and to nothing else. Gold, health, mana, soul points, class rank, quest rewards, quest requirements, item counts, and the numbers that float over an avatar when something takes damage. Chat is not on the list and cannot get onto it, whatever it is holding or however it is written.

The old guess about room numbers is gone with it. It existed to keep `join room 9922` intact in chat, and chat is no longer somewhere this runs.

## Lag when a control deck launches it

Reported by one of Laaiti's friends: slow when launched from a Fifine control deck, fine when the same `aether.exe` is double clicked. Same binary, same machine, same graphics card, so nothing about the rendering explains it.

A process inherits two things from whatever started it, and a control deck is a background helper that has both. Priority class is handed straight down. Power throttling is the one that hurts: Windows 11 puts a process it considers background into a low power mode that parks it on efficiency cores and holds the clocks down, and a child of a background process inherits that. For something drawing frames it reads exactly like a slow computer.

Aether now switches that throttling off for itself at startup and lifts its priority back to normal if it was handed something lower. It does not raise itself above normal in either case.

If this was the cause, the deck and the desktop shortcut should now behave the same. If it is still slower from the deck, it is something else and worth saying so.

## Downloads

`Aether-Setup-0.5.14-win-x64.exe` for the installer, `Aether-Portable-0.5.14-win-x64.zip` if you would rather not install anything.

There may also be `Aether-Launcher-0.5.14-win-x64.exe`. It is the same installer at a few megabytes instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It needs a connection; the other two do not.
