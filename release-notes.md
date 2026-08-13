A pre-release. AQW's own fonts are used for AQW's text, and small panels keep their corners.

## The text is drawn in the wrong typeface

AQW sets its interface in **Mini 7** and **Mini 7 Tight** -- pixel faces, drawn to be crisp at the
sizes it uses them -- and it embeds both in the game: 218 glyphs each, with layout.

Aether never used them. Neither is a system font and neither was meant to be, so asking Windows for
them found Tahoma for one and nothing at all for the other, and every panel of text was drawn in a
typeface the author never chose. From the log of a normal session:

    Loading device font "Tahoma" for "Mini 7" via Tahoma
    Unknown device font "Mini 7 Tight"

That is what the thin, soft text was. Not resolution, not the cache, not antialiasing -- the shapes
the game ships were sitting in its own library, unused, while a different face was substituted at
the same nominal size.

A face the movie carries under the requested name is now preferred over one the system substitutes
under a different one. It is guarded on the embedded font actually covering ordinary text, because a
movie also embeds single-purpose subsets under names it shares with real fonts -- this build carries
an `Arial` of nineteen glyphs, cut for one caption, alongside the genuine article. Preferring that
over the system's Arial would empty out most of the interface.

## Small panels keep their corners too

A shrinking axis was refused outright for three releases, on the theory that dividing a border by a
scale below one magnifies the corner. It does enlarge the band in the object's own space -- which is
exactly how the border arrives at its drawn size once the object's own scale is applied, the same as
when growing.

The artefact that rule was written for turned out to be AQW's own cooldown overlay on the aura
icons, which is supposed to look like that. What the rule actually did was leave every panel drawn
smaller than its authored size with stretched corners, and a tooltip is sized to its text, so that
is most of them -- including the aura tooltips.

Measured on a bordered box drawn with a 12 pixel border, at three sizes:

| | border, no grid | border, now |
|---|---|---|
| half size | 6 px | **12 px** |
| 3x wide, 0.6x tall | 36 / 7 px | **12 / 12 px** |
| 3x | 36 px | **12 px** |

The drawn size, whichever way it is scaled. The one case still refused is the only one that cannot
be drawn: squeezed so far that the two borders alone would not fit.

## Not fixed in this build

The skill icon in its grey button, the drop-accept checkmark and the text inside an aura tooltip all
sit left of centre. AQW centres each by measuring another object -- `x = container.width / 2 -
content.width / 2` -- so this is a reported width rather than anything about drawing, which is why
changing how things are drawn has not moved it. Bounds match Flash on the three counts checked so
far: filters excluded, invisible children included, stroke widths included.

Frame rate still decays over a session: 127,487 offscreen texture allocations totalling 1.37 TB in
one two-minute trace, everything past the forty-second mark evicted for budget as soon as it is
made.

## Downloads

`Aether-Setup-0.6.13-win-x64.exe` for the installer, `Aether-Portable-0.6.13-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.13-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
