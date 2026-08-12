Frame rate work, aimed at the thing that costs the most when a room fills up.

## Crowded rooms

Every player in a room brings their own effects with them, and each one of those was costing far more than the pixels it drew.

When the game asks for a blend, a glow on a weapon or a highlight on a cape, the renderer was setting aside an offscreen image, spending a drawing pass filling it, and then compositing the result. Measured across roughly two million blends in a session, **99% of them wrap exactly one thing**. For those the offscreen image is pure ceremony: the same pixels come out of drawing the thing directly with the blend switched on.

Frame time is almost entirely the number of drawing passes, at somewhere between 50 and 62 microseconds each, so passes are the frame. Those are now skipped for the blends that can skip them, which is every blend of the simple kind, Add, Screen, Subtract and plain layering, wrapped around a single drawn thing. Of the blend modes AQW's own artwork uses, a third are of that kind.

This scales with how many players are on screen, because the blends do.

Two kinds keep their offscreen image. The ones that read what is underneath them, such as Multiply and Overlay, genuinely need the picture gathered somewhere first. And a shape made of several overlapping fills keeps it too, because blending each fill separately is not the same as blending the finished shape: where two fills overlap, doing it the short way would blend twice.

None of this changes what anything looks like. Each case was checked by rendering the same frame with and without the change and comparing the files byte for byte, not by eye.

## A blend no longer assumes its source fills its own image

When a blend composites part of the screen, it samples the image it drew into. That image comes from a pool which hands back something at least as big as was asked for, and often bigger, so the drawn part may occupy only a corner of it. The sampling assumed the two were the same size, which would squash the source into a fraction of itself and read empty padding for the rest.

The sizes happen to line up today, so nothing looks different, and the same frame renders byte for byte identically. It is fixed because it is the kind of thing that is only correct by accident, and the accident is one grid size away from ending.

## Weapon glows

A weapon's glow is not a filter. AQW builds it as a blend, which means the weapon is drawn by combining it with whatever is behind it: the map, the ground, the other characters.

Aether holds each character as a finished image, which is a large part of why crowded rooms run as well as they do. Nothing else can tell the difference, because a finished image laid over the map looks the same as the parts drawn one at a time. A blend can tell. Held as an image, a weapon blends against the empty space inside the character's own picture rather than against the map, and Overlay, which is the blend AQW uses, divides by how solid the thing behind it is. Inside the character's own picture, that is nothing at all.

A character with anything inside it that blends is no longer held as an image. It costs that character the optimisation and keeps everyone else's, which is the right way round: the optimisation is ours and the artwork is theirs.

## Still to come

Dragging the window still costs frames while you are dragging.

## Downloads

`Aether-Setup-0.5.16-win-x64.exe` for the installer, `Aether-Portable-0.5.16-win-x64.zip` if you would rather not install anything.

There may also be `Aether-Launcher-0.5.16-win-x64.exe`. It is the same installer at a few megabytes instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It needs a connection; the other two do not.
