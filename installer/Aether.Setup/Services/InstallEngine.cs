using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32;

namespace Aether.Setup.Services;

/// <summary>Does the actual installing and uninstalling. One engine behind all three front ends — the wizard, a
/// silent run, and the uninstaller — so they can't drift into behaving differently.</summary>
public static class InstallEngine
{
    public const string AppName = "Aether";
    public const string ExeName = "aether.exe";
    public const string ProcessName = "aether";
    public const string UninstallerName = "uninstall.exe";
    public const string ProgId = "Aether.Flash";
    private const string ArpKey = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Aether";
    private const string CapabilitiesKey = @"SOFTWARE\Aether\Capabilities";
    private const string UninstallRoot = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    public static readonly string[] FlashExt = { ".swf" };
    public static readonly string[] ProjectorExt = { ".spl", ".ruf" };

    public sealed record Progress(string Stage, double Fraction);

    /// <summary>Where this build gets Aether's files.</summary>
    public enum FileSource
    {
        /// <summary>Nowhere. This build is the uninstaller.</summary>
        None,
        /// <summary>Compressed inside this executable. The full setup.</summary>
        Embedded,
        /// <summary>Downloaded from the releases page. The launcher.</summary>
        Download,
    }

    /// <summary>Decided by how this copy was built, never by an argument. See SetupInfo.IsLauncher.</summary>
    public static FileSource SourceOfFiles =>
        Payload.Exists ? FileSource.Embedded
        : SetupInfo.IsLauncher ? FileSource.Download
        : FileSource.None;

    /// <summary>Where a downloaded archive is staged.
    ///
    /// The temp folder, not the install folder. A hundred megabyte zip left in Program Files after every
    /// install is litter, and writing into the install folder before the download has been verified would put
    /// unverified bytes where the installed program lives. The version is in the name so a retry after a
    /// failure and a different version in flight cannot collide.</summary>
    public static string StagedArchivePath(string version) =>
        Path.Combine(Path.GetTempPath(), $"Aether-Portable-{version}-win-x64.zip");

    /// <summary>Install. Reports coarse stages with a 0..1 fraction so a bar can move sensibly.</summary>
    public static async Task InstallAsync(InstallOptions o, string version, IProgress<Progress>? progress,
                                          Action<string> log, CancellationToken ct)
    {
        if (o.Validate() is { } why) throw new InvalidOperationException(why);

        progress?.Report(new Progress("Closing Aether", 0));
        await CloseAetherAsync(log, ct);

        // Before a single file is written: retire an MSI-installed Aether if one is there. Doing it after would
        // hand the old uninstaller a folder full of files we had just written. See RemoveLegacyInstall.
        progress?.Report(new Progress("Checking for an older install", 0.02));
        await RemoveLegacyInstallAsync(log, ct);

        if (SourceOfFiles == FileSource.Download)
        {
            await DownloadAndExtractAsync(o, progress, log, ct);
        }
        else
        {
            progress?.Report(new Progress("Copying files", 0));
            var inner = new Progress<double>(f => progress?.Report(new Progress("Copying files", f * 0.85)));
            await Payload.ExtractAsync(o.InstallDir, inner, log, ct);
        }

        progress?.Report(new Progress("Creating shortcuts", 0.9));
        Shortcuts(o, log);

        progress?.Report(new Progress("Registering with Windows", 0.95));
        RegisterFileTypes(o, log);
        RegisterUninstall(o, version, log);

        progress?.Report(new Progress("Done", 1));
        log("Install complete.");
    }

    /// <summary>The launcher's half of the install: ask the releases page what is current, fetch it, unpack it.
    ///
    /// Progress is split so the bar keeps the same shape as an embedded install. Downloading is most of the
    /// wait on any normal connection, so it gets most of the bar.
    ///
    /// Nothing here decides whether the download is trustworthy. <see cref="ReleaseFetcher"/> refuses anything
    /// that does not match the hash the release published, and refuses a release that publishes no hash at
    /// all, so by the time this unpacks there is nothing left to second-guess.</summary>
    private static async Task DownloadAndExtractAsync(InstallOptions o, IProgress<Progress>? progress,
                                                      Action<string> log, CancellationToken ct)
    {
        progress?.Report(new Progress("Checking for the latest version", 0.05));
        using var fetcher = new ReleaseFetcher();

        var release = await fetcher.LatestAsync(ct)
            ?? throw new InvalidOperationException(
                "Could not reach the Aether releases page. Check your connection and try again, or download the portable version instead.");

        log($"Latest release is {release.Version}.");
        string staged = StagedArchivePath(release.Version);

        try
        {
            progress?.Report(new Progress($"Downloading Aether {release.Version}", 0.1));
            var downloading = new Progress<double>(f =>
                progress?.Report(new Progress($"Downloading Aether {release.Version}", 0.1 + f * 0.6)));
            await fetcher.DownloadClientAsync(release, staged, downloading, ct);
            log($"Downloaded and verified {Path.GetFileName(staged)}.");

            progress?.Report(new Progress("Copying files", 0.7));
            var extracting = new Progress<double>(f => progress?.Report(new Progress("Copying files", 0.7 + f * 0.15)));
            await Payload.ExtractZipAsync(staged, o.InstallDir, extracting, log, ct);
        }
        finally
        {
            // Whatever happened, the staged copy has done its job. Leaving a hundred megabytes in the temp
            // folder after a failed install is the kind of thing nobody finds until the disk is full.
            try { if (File.Exists(staged)) File.Delete(staged); }
            catch { /* temp files are the operating system's problem from here */ }
        }
    }

    public static async Task UninstallAsync(string installDir, bool keepSettings, IProgress<Progress>? progress,
                                            Action<string> log, CancellationToken ct)
    {
        progress?.Report(new Progress("Closing Aether", 0));
        await CloseAetherAsync(log, ct);

        progress?.Report(new Progress("Removing Windows entries", 0.15));
        UnregisterFileTypes(log);
        if (OperatingSystem.IsWindows())
            try { Registry.CurrentUser.DeleteSubKeyTree(ArpKey, throwOnMissingSubKey: false); } catch { /* already gone */ }

        progress?.Report(new Progress("Removing shortcuts", 0.25));
        foreach (string lnk in new[] { StartMenuLink(), DesktopLink() })
            try { if (File.Exists(lnk)) { File.Delete(lnk); log($"Removed {lnk}"); } } catch { /* in use */ }

        progress?.Report(new Progress("Removing files", 0.35));
        RemoveInstallDir(installDir, log, progress, ct);

        if (!keepSettings)
        {
            foreach (string data in SettingsDirs())
                try { if (Directory.Exists(data)) { Directory.Delete(data, recursive: true); log($"Removed {data}"); } }
                catch (Exception ex) { log("Couldn't remove settings: " + ex.Message); }
        }
        else log("Kept your settings and saved games.");

        progress?.Report(new Progress("Done", 1));
    }

    /// <summary>%AppData%\Aether holds preferences; %LocalAppData%\aether holds the downloaded OpenH264 codec and
    /// the Flash local-storage that AQW writes save data into. Both are Aether's, both go together.</summary>
    private static IEnumerable<string> SettingsDirs()
    {
        yield return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), AppName);
        yield return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "aether");
    }

    /// <summary>Ask a running Aether to close and wait for it. Never kills outright — a preferences write or a
    /// Flash local-storage flush could be in flight, and this isn't urgent enough to lose someone's data over.</summary>
    public static async Task CloseAetherAsync(Action<string> log, CancellationToken ct)
    {
        var procs = Running();
        if (procs.Length == 0) return;
        log($"Aether is running ({procs.Length} process) — asking it to close.");
        foreach (var p in procs)
            try { p.CloseMainWindow(); } catch { /* already exiting */ }

        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(15);
        while (DateTime.UtcNow < deadline)
        {
            if (Running().Length == 0) { await Task.Delay(400, ct); return; }
            await Task.Delay(200, ct);
        }
        throw new InvalidOperationException("Aether is still running. Close it and try again.");
    }

    public static Process[] Running()
    {
        try { return Process.GetProcessesByName(ProcessName); }
        catch { return Array.Empty<Process>(); }
    }

    /// <summary>Where OUR Aether is installed, or null.
    ///
    /// Deliberately does NOT fall back to a machine-wide MSI install. It used to, so the wizard would offer an
    /// in-place update — but now that setup runs unelevated, pre-filling C:\Program Files\Aether would hand
    /// the user a path they cannot write to and a validation error they did nothing to cause. The old install
    /// is still noticed; it just gets retired by <see cref="RemoveLegacyInstallAsync"/> rather than reused.</summary>
    public static string? ExistingInstall()
        => ReadOurArp("InstallLocation") is { } a && Directory.Exists(a) ? a : null;

    public static string? ExistingVersion()
        => ReadOurArp("DisplayVersion") ?? FindLegacy()?.Version;

    /// <summary>Read one of OUR Add/Remove Programs values. HKCU only, on purpose: a machine-wide entry left
    /// by an older elevated build must not pre-fill the wizard, because setup now runs unelevated and would
    /// offer a Program Files path it then has to reject. Those are found by FindLegacy instead.</summary>
    private static string? ReadOurArp(string name)
    {
        if (!OperatingSystem.IsWindows()) return null;
        try
        {
            using var k = Registry.CurrentUser.OpenSubKey(ArpKey);
            if (k?.GetValue(name) is string s && s.Length > 0) return s;
        }
        catch { /* not installed for this user */ }
        return null;
    }


    private sealed record LegacyEntry(string ProductCode, string? InstallLocation, string? Version, string? UninstallString);

    /// <summary>Find an Aether registered by the MSI/WiX package under <c>packaging\</c>, which is how the
    /// existing C:\Program Files\Aether installs got there.
    ///
    /// Discovered rather than hardcoded on purpose: an MSI's ARP key is its ProductCode, and that GUID is
    /// regenerated on every build (the .wxs uses <c>Id='*'</c>), so there is no single constant to look for.
    /// Matching on DisplayName across both registry views finds them all.</summary>
    private static LegacyEntry? FindLegacy()
    {
        if (!OperatingSystem.IsWindows()) return null;
        foreach (RegistryView view in new[] { RegistryView.Registry64, RegistryView.Registry32 })
        {
            try
            {
                using var hklm = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, view);
                using var root = hklm.OpenSubKey(UninstallRoot);
                if (root is null) continue;
                foreach (string name in root.GetSubKeyNames())
                {

                    using var k = root.OpenSubKey(name);
                    if (k?.GetValue("DisplayName") as string is not { } display) continue;
                    if (!display.Equals(AppName, StringComparison.OrdinalIgnoreCase)) continue;
                    return new LegacyEntry(name, k.GetValue("InstallLocation") as string,
                                           k.GetValue("DisplayVersion") as string,
                                           k.GetValue("UninstallString") as string);
                }
            }
            catch { /* unreadable view */ }
        }
        return null;
    }

    /// <summary>Retire an MSI-installed Aether properly, by asking Windows Installer to remove it, rather than
    /// deleting its registry key and leaving a half-registered product behind. Runs BEFORE we write anything:
    /// leaving it would show a second Aether in Add or remove programs whose uninstaller deletes the copy we
    /// just wrote, and running it afterwards would do exactly that.</summary>
    private static async Task RemoveLegacyInstallAsync(Action<string> log, CancellationToken ct)
    {
        if (!OperatingSystem.IsWindows() || FindLegacy() is not { } legacy) return;
        log($"Found an existing {AppName} {legacy.Version} installed by the older package — removing it first.");

        bool isMsi = legacy.ProductCode.StartsWith('{')
                     && (legacy.UninstallString?.Contains("msiexec", StringComparison.OrdinalIgnoreCase) ?? false);
        if (isMsi)
        {
            try
            {
                var psi = new ProcessStartInfo("msiexec.exe", $"/x {legacy.ProductCode} /qn /norestart")
                {
                    UseShellExecute = false, CreateNoWindow = true,
                };
                using var p = Process.Start(psi);
                if (p is not null)
                {
                    await p.WaitForExitAsync(ct);
                    log($"msiexec removed the previous package (exit {p.ExitCode}).");
                    if (p.ExitCode == 0) return;
                }
            }
            catch (Exception ex) { log("Couldn't run msiexec: " + ex.Message); }
        }

        // Not an MSI, or msiexec declined — most likely because removing a machine-wide package needs
        // administrator and setup deliberately doesn't ask for it. Not fatal: the new install goes to a
        // different, per-user folder, so the old one isn't in the way, it's just still listed. Say so
        // plainly rather than failing an install that is otherwise fine.
        log($"The older machine-wide {AppName} at {legacy.InstallLocation ?? "an unknown folder"} is still " +
            "installed. Remove it from Windows' \"Installed apps\" if you want only one copy — it needs " +
            "administrator rights, which this setup doesn't ask for.");
    }

    // ---- pieces ------------------------------------------------------------------------------------------

    private static void RemoveInstallDir(string dir, Action<string> log, IProgress<Progress>? progress, CancellationToken ct)
    {
        if (!Directory.Exists(dir)) return;
        // The uninstaller is running FROM this folder, so it can't delete itself. Everything else goes now, and
        // the executable is scheduled for removal on the next reboot.
        string self = SelfPath();
        var files = Directory.GetFiles(dir, "*", SearchOption.AllDirectories);
        for (int i = 0; i < files.Length; i++)
        {
            ct.ThrowIfCancellationRequested();
            if (PathEquals(files[i], self)) continue;
            try { File.Delete(files[i]); } catch { /* locked — leave it */ }
            if (i % 40 == 0) progress?.Report(new Progress("Removing files", 0.35 + 0.55 * i / Math.Max(1, files.Length)));
        }
        foreach (string sub in Directory.GetDirectories(dir, "*", SearchOption.AllDirectories).OrderByDescending(d => d.Length))
            try { if (!Directory.EnumerateFileSystemEntries(sub).Any()) Directory.Delete(sub); } catch { }
        try { if (!Directory.EnumerateFileSystemEntries(dir).Any()) Directory.Delete(dir); }
        catch { log("The install folder will be removed on the next restart."); ScheduleDeleteOnReboot(self, dir); }
    }

    private static void Shortcuts(InstallOptions o, Action<string> log)
    {
        string exe = Path.Combine(o.InstallDir, ExeName);
        if (o.StartMenuShortcut) MakeLink(StartMenuLink(), exe, o.InstallDir, log); else Remove(StartMenuLink());
        if (o.DesktopShortcut) MakeLink(DesktopLink(), exe, o.InstallDir, log); else Remove(DesktopLink());

        void Remove(string p) { try { if (File.Exists(p)) File.Delete(p); } catch { } }
    }

    private static string StartMenuLink() => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.StartMenu), "Programs", "Aether.lnk");
    private static string DesktopLink() => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory), "Aether.lnk");

    private static void MakeLink(string linkPath, string target, string workingDir, Action<string> log)
    {
        if (!OperatingSystem.IsWindows()) return;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(linkPath)!);
            var type = Type.GetTypeFromProgID("WScript.Shell");
            if (type is null) { log("Shell scripting host unavailable — skipped a shortcut."); return; }
            dynamic shell = Activator.CreateInstance(type)!;
            dynamic link = shell.CreateShortcut(linkPath);
            link.TargetPath = target;
            link.WorkingDirectory = workingDir;
            link.IconLocation = target + ",0";
            link.Description = "Aether";
            link.Save();
            log($"Shortcut: {linkPath}");
        }
        catch (Exception ex) { log($"Couldn't create {Path.GetFileName(linkPath)}: {ex.Message}"); }
    }

    private static void RegisterFileTypes(InstallOptions o, Action<string> log)
    {
        if (!OperatingSystem.IsWindows()) return;
        string exe = Path.Combine(o.InstallDir, ExeName);
        var wanted = new List<string>();
        if (o.AssociateFlash) wanted.AddRange(FlashExt);
        if (o.AssociateProjector) wanted.AddRange(ProjectorExt);

        try
        {
            using (var appPaths = Registry.CurrentUser.CreateSubKey(
                       @"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\" + ExeName))
            {
                appPaths.SetValue("", exe);
                appPaths.SetValue("Path", o.InstallDir);
            }

            if (wanted.Count == 0) { UnregisterFileTypes(log); return; }

            // A ProgID plus a Capabilities entry — the pair Windows needs before Aether can show up in Default
            // Apps at all. Deliberately OpenWithProgids rather than seizing each extension's default: taking
            // over someone's file types without asking is what makes installers resented.
            using (var progId = Registry.CurrentUser.CreateSubKey(@"SOFTWARE\Classes\" + ProgId))
            {
                progId.SetValue("", "Flash Movie");
                using (var icon = progId.CreateSubKey("DefaultIcon")) icon.SetValue("", exe + ",0");
                using var cmd = progId.CreateSubKey(@"shell\open\command");
                cmd.SetValue("", $"\"{exe}\" \"%1\"");
            }

            using (var caps = Registry.CurrentUser.CreateSubKey(CapabilitiesKey))
            {
                caps.SetValue("ApplicationName", AppName);
                caps.SetValue("ApplicationIcon", exe + ",0");
                caps.SetValue("ApplicationDescription", "An AQW-optimized Flash client.");
                using var fa = caps.CreateSubKey("FileAssociations");
                foreach (string ext in wanted) fa.SetValue(ext, ProgId);
            }
            using (var reg = Registry.CurrentUser.CreateSubKey(@"SOFTWARE\RegisteredApplications"))
                reg.SetValue(AppName, CapabilitiesKey);

            foreach (string ext in wanted)
                using (var owp = Registry.CurrentUser.CreateSubKey($@"SOFTWARE\Classes\{ext}\OpenWithProgids"))
                    owp.SetValue(ProgId, Array.Empty<byte>(), RegistryValueKind.None);

            log($"Registered {wanted.Count} file types (Aether appears in Open with, and in Default apps).");
        }
        catch (Exception ex) { log("File-type registration failed: " + ex.Message); }
    }

    private static void UnregisterFileTypes(Action<string> log)
    {
        if (!OperatingSystem.IsWindows()) return;
        try
        {
            foreach (string ext in FlashExt.Concat(ProjectorExt))
                try
                {
                    using var owp = Registry.CurrentUser.OpenSubKey($@"SOFTWARE\Classes\{ext}\OpenWithProgids", writable: true);
                    owp?.DeleteValue(ProgId, throwOnMissingValue: false);
                }
                catch { /* not registered */ }

            Registry.CurrentUser.DeleteSubKeyTree(@"SOFTWARE\Classes\" + ProgId, throwOnMissingSubKey: false);
            Registry.CurrentUser.DeleteSubKeyTree(@"SOFTWARE\Aether", throwOnMissingSubKey: false);
            try
            {
                using var reg = Registry.CurrentUser.OpenSubKey(@"SOFTWARE\RegisteredApplications", writable: true);
                reg?.DeleteValue(AppName, throwOnMissingValue: false);
            }
            catch { }
            try
            {
                using var ap = Registry.CurrentUser.OpenSubKey(
                    @"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths", writable: true);
                ap?.DeleteSubKeyTree(ExeName, throwOnMissingSubKey: false);
            }
            catch { }
            log("Removed the file-type registrations.");
        }
        catch (Exception ex) { log("Couldn't fully remove the registrations: " + ex.Message); }
    }

    /// <summary>The Add/Remove Programs entry, so Aether uninstalls like any other app.</summary>
    private static void RegisterUninstall(InstallOptions o, string version, Action<string> log)
    {
        if (!OperatingSystem.IsWindows()) return;
        try
        {
            string exe = Path.Combine(o.InstallDir, ExeName);
            string uninst = Path.Combine(o.InstallDir, UninstallerName);
            long size = 0;
            try { size = new DirectoryInfo(o.InstallDir).GetFiles("*", SearchOption.AllDirectories).Sum(f => f.Length); }
            catch { /* size is a nicety */ }

            using var k = Registry.CurrentUser.CreateSubKey(ArpKey);
            k.SetValue("DisplayName", AppName);
            k.SetValue("DisplayVersion", version);
            k.SetValue("Publisher", AppName);
            k.SetValue("DisplayIcon", exe + ",0");
            k.SetValue("InstallLocation", o.InstallDir);
            k.SetValue("UninstallString", $"\"{uninst}\" --uninstall");
            k.SetValue("QuietUninstallString", $"\"{uninst}\" --uninstall --silent");
            k.SetValue("EstimatedSize", (int)(size / 1024), RegistryValueKind.DWord);
            k.SetValue("NoModify", 1, RegistryValueKind.DWord);
            k.SetValue("NoRepair", 1, RegistryValueKind.DWord);
            k.SetValue("InstallDate", DateTime.Now.ToString("yyyyMMdd"));
            log("Registered in Add or remove programs.");
        }
        catch (Exception ex) { log("Couldn't register the uninstall entry: " + ex.Message); }
    }

    private static void ScheduleDeleteOnReboot(string file, string dir)
    {
        // MoveFileEx with MOVEFILE_DELAY_UNTIL_REBOOT is the only way to remove a running executable.
        try { MoveFileEx(file, null, 4); MoveFileEx(dir, null, 4); } catch { /* best effort */ }
    }

    [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode, SetLastError = true)]
    private static extern bool MoveFileEx(string existing, string? newName, int flags);

    private static string SelfPath()
    {
        try { return Process.GetCurrentProcess().MainModule?.FileName ?? ""; }
        catch { return ""; }
    }

    private static bool PathEquals(string a, string b)
    {
        if (a.Length == 0 || b.Length == 0) return false;
        try { return string.Equals(Path.GetFullPath(a), Path.GetFullPath(b), StringComparison.OrdinalIgnoreCase); }
        catch { return false; }
    }
}
