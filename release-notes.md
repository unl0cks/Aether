A pre-release. The toolbar buttons should keep their corners whatever icon is on them.

## Why some buttons kept their corners and others did not

The nine pieces are worked out by measuring the grid against the object's edges, so those edges have
to be the ones the grid was drawn against -- the border art, and nothing else.

They were not. They were the object's ordinary bounds, which are the union of *every* child, so an
icon that reaches past the frame it sits in dragged the measured edge out with it and moved every
band. Two buttons skinned by the same frame then sliced differently depending on what was sitting on
them. That is why the third and fourth skill buttons behaved unlike the first, second and fifth: not
the frame, the icon on it.

Worked through on a bordered box at three times its size, with a child reaching 14 pixels past the
frame: the right border ran from 88 to 100 in the artwork, and was being drawn into 105 to 114
instead. Measured, with the child overflowing:

| | border | contents at |
|---|---|---|
| no grid | 36 px | 96, 96 |
| grid, now | **12 px** | **72, 72** |

Which is band for band what the same box measures with no overflowing child at all. The art's edges
are measured now, so what is sitting on top cannot move the border behind it.

## What the aura icons turned out not to be

The buff and aura icons are **not** being sliced, and have not been since 0.6.9. AQW builds each one
at `scaleX = scaleY = 0.6`, and slicing has declined anything placed smaller than it was drawn since
that build -- so three releases of scaling-grid work could not have been what moves them.

They are positioned by measurement rather than by transform. From AQW's own `playerAuras`:

    _loc10_.scaleX = _loc10_.scaleY = 34 / _loc10_.width;
    _loc10_.x = bg.width / 2 - _loc10_.width / 2;
    _loc10_.y = bg.height / 2 - _loc10_.height / 2;

The icon is centred using the reported width of two other objects. If a reported width is off, the
icon is off, and no amount of drawing changes will move it. Ruled out so far: bounds correctly
exclude filters, and correctly include invisible children, both matching Flash. That is where this
now goes, and it is a different fault from the one the last three builds were aimed at.

## Text

Still open, and the mechanism is now known rather than guessed. `DefineCSMTextSettings` is how a
Flash author says how text should be filled. Counted on the live build with
`cargo run -p swf --example text_settings_census`: **627 text fields, every one of them asking for
advanced antialiasing**, 500 with sub-pixel grid fitting and 105 with pixel grid fitting. Aether
parses all of it, stores it on the text object, and draws the glyph outline plainly regardless.
Grid fitting is what puts a glyph's stems on whole pixels instead of across two, which is the
difference between text that reads solid and text that reads thin.

## Frame rate over time

Measured from a frame trace rather than described. In one two-minute session the offscreen texture
pool made **127,487 allocations totalling 1.37 TB**, and there is a sharp change partway through:
targets are 0.13 to 0.5 MB each for the first forty seconds and 9 to 18 MB each after that, at which
point *every* allocation is immediately evicted for budget -- 1,808 allocated and 1,808 evicted in a
single interval, 30 GB of churn, while the pool holds a flat 121 MB.

So the collapse is a retention budget that cannot hold the targets a full-screen effect asks for.
The budget has since been raised to 512 MiB and that trace predates it, so the numbers above are not
today's; raising it further is not the answer either, because 768 MiB per pool has already been
measured pushing a 10 GB card into a device-lost fault. The real cost is how many full-viewport
targets one frame asks for. That is the next thing, and it needs a fresh trace to size properly.

## Downloads

`Aether-Setup-0.6.11-win-x64.exe` for the installer, `Aether-Portable-0.6.11-win-x64.zip` if you
would rather not install anything.

There may also be `Aether-Launcher-0.6.11-win-x64.exe`. It is the same installer at a few megabytes
instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It
needs a connection; the other two do not.
