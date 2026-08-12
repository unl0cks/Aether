Frame rate work, aimed at the thing that costs the most when a room fills up.

## Crowded rooms

Every player in a room brings their own effects with them, and each one of those was costing far more than the pixels it drew.

When the game asks for a blend, a glow on a weapon or a highlight on a cape, the renderer was setting aside an offscreen image, spending a drawing pass filling it, and then compositing the result. Measured across roughly two million blends in a session, **99% of them wrap exactly one thing**. For those the offscreen image is pure ceremony: the same pixels come out of drawing the thing directly with the blend switched on.

Frame time is almost entirely the number of drawing passes, at somewhere between 50 and 62 microseconds each, so passes are the frame. Those are now skipped for the blends that can skip them, which is every blend of the simple kind, Add, Screen, Subtract and plain layering, wrapped around a single drawn thing. Of the blend modes AQW's own artwork uses, a third are of that kind.

This scales with how many players are on screen, because the blends do.

Two kinds keep their offscreen image. The ones that read what is underneath them, such as Multiply and Overlay, genuinely need the picture gathered somewhere first. And a shape made of several overlapping fills keeps it too, because blending each fill separately is not the same as blending the finished shape: where two fills overlap, doing it the short way would blend twice.

None of this changes what anything looks like. Each case was checked by rendering the same frame with and without the change and comparing the files byte for byte, not by eye.

## Still to come

Dragging the window still costs frames while you are dragging, and the weapon glow is still wrong. Both are known and neither is forgotten; the glow in particular turned out not to be what everyone assumed, which is written up below.

## Downloads

`Aether-Setup-0.5.15-win-x64.exe` for the installer, `Aether-Portable-0.5.15-win-x64.zip` if you would rather not install anything.

There may also be `Aether-Launcher-0.5.15-win-x64.exe`. It is the same installer at a few megabytes instead of a hundred, because it downloads Aether while it installs rather than carrying a copy. It needs a connection; the other two do not.
