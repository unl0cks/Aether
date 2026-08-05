using System;
using System.IO;
using System.Linq;

namespace Aether.Setup.Services;

/// <summary>What the user chose. The same object whether it came from the wizard, from a silent run driven by
/// another tool (passed on the command line), or from defaults — so every route installs identically.</summary>
public sealed class InstallOptions
{
    public string InstallDir { get; set; } = DefaultInstallDir;
    public bool DesktopShortcut { get; set; }
    public bool StartMenuShortcut { get; set; } = true;
    public bool AssociateFlash { get; set; } = true;
    public bool AssociateProjector { get; set; } = true;
    public bool LaunchWhenDone { get; set; } = true;

    /// <summary>%LocalAppData%\Programs\Aether, not Program Files. A per-user install writes only where the
    /// person running it can already write, so setup needs no elevation at all — which is the difference
    /// between a UAC prompt on every install and none. Same place VS Code, Discord and Chrome default to.</summary>
    public static string DefaultInstallDir => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "Aether");

    /// <summary>Render as arguments for a silent run.</summary>
    public string ToArguments(string progressFile) =>
        $"--silent --dir \"{InstallDir}\" --progress \"{progressFile}\"" +
        (DesktopShortcut ? " --desktop" : "") +
        (StartMenuShortcut ? " --startmenu" : "") +
        (AssociateFlash ? " --assoc-flash" : "") +
        (AssociateProjector ? " --assoc-projector" : "");

    public static InstallOptions FromArguments(string[] args)
    {
        var o = new InstallOptions
        {
            // Explicit flags mean explicit choices: in silent mode nothing is on unless it was asked for.
            StartMenuShortcut = false, AssociateFlash = false, AssociateProjector = false, LaunchWhenDone = false,
        };
        for (int i = 0; i < args.Length; i++)
            switch (args[i].ToLowerInvariant())
            {
                case "--dir" when i + 1 < args.Length: o.InstallDir = args[++i]; break;
                case "--desktop": o.DesktopShortcut = true; break;
                case "--startmenu": o.StartMenuShortcut = true; break;
                case "--assoc-flash": o.AssociateFlash = true; break;
                case "--assoc-projector": o.AssociateProjector = true; break;
                case "--launch": o.LaunchWhenDone = true; break;
            }
        if (string.IsNullOrWhiteSpace(o.InstallDir)) o.InstallDir = DefaultInstallDir;
        return o;
    }

    /// <summary>Why this directory won't do, or null if it will. Checked before a single file is written.</summary>
    public string? Validate()
    {
        string dir = InstallDir?.Trim() ?? "";
        if (dir.Length == 0) return "Choose a folder to install into.";
        if (dir.IndexOfAny(Path.GetInvalidPathChars()) >= 0) return "That path contains characters Windows won't allow.";
        try
        {
            string full = Path.GetFullPath(dir);
            if (!Path.IsPathRooted(full)) return "Use a full path, starting with a drive letter.";
            // Installing directly into a drive root or into Windows itself is almost never meant, and both are
            // nasty to undo — the uninstaller would be pointed at a folder full of things that aren't ours.
            if (Path.GetPathRoot(full)!.Equals(full.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar,
                                               StringComparison.OrdinalIgnoreCase))
                return "Pick a folder inside the drive, not the drive itself.";
            string win = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
            if (full.StartsWith(win, StringComparison.OrdinalIgnoreCase)) return "Not inside the Windows folder.";
            if (Directory.Exists(full) && Directory.EnumerateFileSystemEntries(full).Any()
                && !File.Exists(Path.Combine(full, InstallEngine.ExeName)))
                return "That folder isn't empty and doesn't already contain Aether.";
            // Setup runs unelevated, so a machine-wide folder has to be caught here rather than discovered
            // halfway through extracting, with some of the files already written.
            if (!CanWriteInto(full))
                return "Aether can't write there. Pick a folder you own, such as the default one.";
        }
        catch (Exception ex) { return ex.Message; }
        return null;
    }

    /// <summary>Can we actually create files here? Asked by writing one, because Windows' answer depends on
    /// the full ACL and on virtualisation, and nothing short of trying is reliable. Walks up to the nearest
    /// existing ancestor, since the target itself usually doesn't exist yet.</summary>
    private static bool CanWriteInto(string path)
    {
        try
        {
            var dir = new DirectoryInfo(path);
            while (dir is not null && !dir.Exists) dir = dir.Parent;
            if (dir is null) return false;

            string probe = Path.Combine(dir.FullName, $".aether-write-test-{Environment.ProcessId}");
            using (File.Create(probe, 1, FileOptions.DeleteOnClose)) { }
            return true;
        }
        catch { return false; }
    }
}
