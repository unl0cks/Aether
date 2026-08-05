using System;
using System.Runtime.InteropServices;
using Avalonia.Controls;

namespace Aether.Setup.Platform;

/// <summary>Windows 11 DWM system backdrop behind a chromeless window, plus immersive dark mode.
/// ACRYLIC (DWMSBT_TRANSIENTWINDOW) blurs what is REALLY behind the window — other apps, the desktop, live
/// content — whereas MICA (DWMSBT_MAINWINDOW) only samples the wallpaper. Setup wants the former: it usually
/// opens over whatever you were doing, and the point is that it sits IN your desktop rather than on top of it.
///
/// The frame must be extended across the whole client area first ("sheet of glass") or the backdrop won't show
/// through. Pair with Background="Transparent" and TransparencyLevelHint="AcrylicBlur, Transparent, None" —
/// note NO Mica in that list: Avalonia tries the hints in order and a Mica win would override the acrylic with
/// wallpaper-only sampling. No-ops off Windows 11.</summary>
public static class DwmAcrylic
{
    private const int DWMWA_USE_IMMERSIVE_DARK_MODE = 20;
    private const int DWMWA_SYSTEMBACKDROP_TYPE = 38;
    private const int DWMWA_WINDOW_CORNER_PREFERENCE = 33;
    private const int DWMWCP_ROUND = 2;

    /// <summary>DWM_SYSTEMBACKDROP_TYPE values.</summary>
    public enum Backdrop { None = 1, Mica = 2, Acrylic = 3, Tabbed = 4 }

    [StructLayout(LayoutKind.Sequential)]
    private struct MARGINS { public int L, R, T, B; }

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);

    [DllImport("dwmapi.dll")]
    private static extern int DwmExtendFrameIntoClientArea(IntPtr hwnd, ref MARGINS margins);

    public static void Apply(Window window, Backdrop backdrop = Backdrop.Acrylic, bool dark = true)
    {
        if (window.TryGetPlatformHandle() is { } h) Apply(h.Handle, backdrop, dark);
    }

    public static void Apply(IntPtr hwnd, Backdrop backdrop = Backdrop.Acrylic, bool dark = true)
    {
        if (!OperatingSystem.IsWindows() || hwnd == IntPtr.Zero) return;
        try
        {
            int d = dark ? 1 : 0;
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, ref d, sizeof(int));
            var m = new MARGINS { L = -1, R = -1, T = -1, B = -1 };   // sheet of glass across the whole window
            DwmExtendFrameIntoClientArea(hwnd, ref m);
            int b = (int)backdrop;
            DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, ref b, sizeof(int));
            int c = DWMWCP_ROUND;
            DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, ref c, sizeof(int));
        }
        catch { /* pre-Win11: no system-backdrop API, the translucent fallback still reads fine */ }
    }
}
