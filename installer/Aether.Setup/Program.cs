using System;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using Avalonia;
using Aether.Setup.Services;

namespace Aether.Setup;

internal sealed class Program
{
    public static SetupArgs Args { get; private set; } = new();

    /// <summary>Whether this run removes Aether rather than installing it. Either asked for — <c>--uninstall</c>,
    /// which is what the Add/Remove Programs entry passes — or IMPLIED: a build with no payload cannot install
    /// anything, so it is the uninstaller and nothing else. That second form is how uninstall.exe works: the
    /// build script publishes a payload-less copy of this exe into the payload, so removing Aether never
    /// depends on the original setup.exe still being around.</summary>
    public static bool Uninstalling => Args.Uninstall || !Payload.Exists;

    /// <summary>%AppData%\Aether\setup.log — the only place a startup failure can surface. This is a WinExe, so
    /// there's no console: an exception before the window appears exits silently and looks like nothing
    /// happened. Breadcrumbs as well as crashes, so a silent death still says how far it got.</summary>
    public static string LogPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "Aether", "setup.log");

    public static void Note(string message) => WriteLine(message);
    public static void Crash(string stage, Exception? ex) => WriteLine($"{stage}: {ex}");

    private static void WriteLine(string text)
    {
        try
        {
            string? dir = Path.GetDirectoryName(LogPath);
            if (dir is not null) Directory.CreateDirectory(dir);
            File.AppendAllText(LogPath, $"[{DateTime.Now:HH:mm:ss.fff}] {text}" + Environment.NewLine);
        }
        catch { /* nowhere left to report */ }
    }

    [STAThread]
    public static int Main(string[] args)
    {
        AppDomain.CurrentDomain.UnhandledException += (_, e) => Crash("unhandled", e.ExceptionObject as Exception);
        Note("---- start ---- " + string.Join(' ', args));
        try { return Run(args); }
        catch (Exception ex) { Crash("startup", ex); throw; }
    }

    private static int Run(string[] args)
    {
        Args = SetupArgs.Parse(args);
        Note("args parsed");

        // Silent mode exists so an outer tool can present the install in ITS window: it collects the options,
        // starts this elevated (elevation can't be combined with redirected output), and follows the progress
        // file we write here. No window of our own, one line per event, exit code says how it went.
        if (Args.Silent) return RunSilent().GetAwaiter().GetResult();

        Note("starting the window");
        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
        Note("closed");
        return 0;
    }

    public static AppBuilder BuildAvaloniaApp()
        => AppBuilder.Configure<App>().UsePlatformDetect().WithInterFont().LogToTrace();

    private static async Task<int> RunSilent()
    {
        var reporter = new ProgressFile(Args.ProgressFile);
        try
        {
            var ct = CancellationToken.None;
            if (Uninstalling)
            {
                string dir = Args.InstallDir ?? InstallEngine.ExistingInstall() ?? AppContext.BaseDirectory;
                await InstallEngine.UninstallAsync(dir, Args.KeepSettings,
                    new Progress<InstallEngine.Progress>(reporter.Stage), reporter.Log, ct);
            }
            else
            {
                await InstallEngine.InstallAsync(InstallOptions.FromArguments(Args.Raw), SetupInfo.Version,
                    new Progress<InstallEngine.Progress>(reporter.Stage), reporter.Log, ct);
            }
            reporter.Done();
            return 0;
        }
        catch (Exception ex)
        {
            reporter.Fail(ex.Message);
            return 1;
        }
    }
}

/// <summary>Append-only progress an outer tool can tail. A file rather than stdout because elevating a process
/// (ShellExecute "runas") and redirecting its streams are mutually exclusive on Windows.</summary>
internal sealed class ProgressFile(string? path)
{
    private readonly object _lock = new();

    public void Stage(InstallEngine.Progress p) => Write($"PROGRESS {p.Fraction.ToString("0.000", System.Globalization.CultureInfo.InvariantCulture)} {p.Stage}");
    public void Log(string line) => Write("LOG " + line);
    public void Done() => Write("DONE");
    public void Fail(string message) => Write("FAIL " + message.Replace('\n', ' '));

    private void Write(string line)
    {
        if (string.IsNullOrEmpty(path)) return;
        try
        {
            lock (_lock)
            {
                Directory.CreateDirectory(Path.GetDirectoryName(path)!);
                File.AppendAllText(path, line + "\n");
            }
        }
        catch { /* the install matters more than the commentary */ }
    }
}

public sealed class SetupArgs
{
    public bool Silent { get; private init; }
    public bool Uninstall { get; private init; }
    public bool KeepSettings { get; private init; }
    public string? InstallDir { get; private init; }
    public string? ProgressFile { get; private init; }
    public string[] Raw { get; private init; } = Array.Empty<string>();

    public static SetupArgs Parse(string[] args)
    {
        bool silent = false, uninstall = false, keep = false;
        string? dir = null, progress = null;
        for (int i = 0; i < args.Length; i++)
            switch (args[i].ToLowerInvariant())
            {
                case "--silent" or "/silent" or "/verysilent": silent = true; break;
                case "--uninstall" or "/uninstall": uninstall = true; break;
                case "--keep-settings": keep = true; break;
                case "--dir" when i + 1 < args.Length: dir = args[i + 1]; break;
                case "--progress" when i + 1 < args.Length: progress = args[i + 1]; break;
            }
        return new SetupArgs
        {
            Silent = silent, Uninstall = uninstall, KeepSettings = keep,
            InstallDir = dir, ProgressFile = progress, Raw = args,
        };
    }
}

/// <summary>Facts about this installer, read off its own assembly so the version can't drift from the build.</summary>
public static class SetupInfo
{
    public static string Version { get; } = Read();

    private static string Read()
    {
        var asm = typeof(SetupInfo).Assembly;
        string v = asm.GetCustomAttribute<System.Reflection.AssemblyInformationalVersionAttribute>()?.InformationalVersion
                   ?? asm.GetName().Version?.ToString() ?? "0.0.0";
        int plus = v.IndexOf('+');
        return plus > 0 ? v[..plus] : v;
    }

    private static T? GetCustomAttribute<T>(this System.Reflection.Assembly a) where T : Attribute =>
        (T?)Attribute.GetCustomAttribute(a, typeof(T));
}
