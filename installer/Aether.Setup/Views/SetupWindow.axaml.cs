using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Media;
using Avalonia.Media.Imaging;
using Avalonia.Media.Transformation;
using Avalonia.Platform;
using Avalonia.Platform.Storage;
using Avalonia.Threading;
using Aether.Setup.Platform;
using Aether.Setup.Services;

namespace Aether.Setup.Views;

/// <summary>The installer's own window — the same glass, type and motion as the client, because a setup that
/// looks like a stock wizard is the first thing a user sees of Aether and it shouldn't look like someone else's
/// software. Three pages: options, working, finished; the same window slides between them.</summary>
public partial class SetupWindow : Window
{
    private readonly StringBuilder _log = new();
    private readonly InstallOptions _options = new();
    private CancellationTokenSource? _cts;
    private Page _page = Page.Options;
    private bool _closing;
    private readonly bool _uninstalling = Program.Uninstalling;
    private string _installDir = "";

    private enum Page { Options, Working, Done }

    public SetupWindow()
    {
        InitializeComponent();

        Opened += (_, _) =>
        {
            try { DwmAcrylic.Apply(this, DwmAcrylic.Backdrop.Acrylic); } catch { /* pre-Win11 */ }
            try { Icon = new WindowIcon(new Bitmap(AssetLoader.Open(new Uri("avares://Aether.Setup/Assets/Aether.png")))); }
            catch { /* default mark */ }
            RootCard.RenderTransform = TransformOperations.Parse("scale(0.97)");
            Opacity = 1;
            Dispatcher.UIThread.Post(() => RootCard.RenderTransform = TransformOperations.Parse("scale(1)"));
        };

        TitleBar.PointerPressed += (_, e) => { if (e.GetCurrentPoint(this).Properties.IsLeftButtonPressed) BeginMoveDrag(e); };
        MinButton.Click += (_, _) => WindowState = WindowState.Minimized;
        CloseButton.Click += (_, _) => Close();
        DetailsToggle.Click += (_, _) =>
        {
            LogPanel.IsVisible = !LogPanel.IsVisible;
            DetailsToggle.Content = LogPanel.IsVisible ? "Hide details" : "Show details";
            if (LogPanel.IsVisible) Dispatcher.UIThread.Post(LogScroll.ScrollToEnd, DispatcherPriority.Loaded);
        };
        BrowseButton.Click += async (_, _) => await PickFolder();
        NextButton.Click += async (_, _) => await OnPrimary();
        BackButton.Click += (_, _) => { if (_cts is { } c && !c.IsCancellationRequested) c.Cancel(); else Close(); };
        DirBox.PropertyChanged += (_, e) => { if (e.Property == TextBox.TextProperty) ValidateDir(); };

        HookGrip(GripNW, WindowEdge.NorthWest); HookGrip(GripN, WindowEdge.North);
        HookGrip(GripNE, WindowEdge.NorthEast); HookGrip(GripW, WindowEdge.West);
        HookGrip(GripE, WindowEdge.East); HookGrip(GripSW, WindowEdge.SouthWest);
        HookGrip(GripS, WindowEdge.South); HookGrip(GripSE, WindowEdge.SouthEast);

        InitFields();
    }

    protected override void OnClosing(WindowClosingEventArgs e)
    {
        if (_closing) { base.OnClosing(e); return; }
        _closing = true;
        e.Cancel = true;
        _cts?.Cancel();
        Opacity = 0;
        RootCard.RenderTransform = TransformOperations.Parse("scale(0.97)");
        DispatcherTimer.RunOnce(Close, TimeSpan.FromMilliseconds(150));
    }

    private void HookGrip(Border grip, WindowEdge edge) =>
        grip.PointerPressed += (_, e) =>
        {
            if (!e.GetCurrentPoint(this).Properties.IsLeftButtonPressed) return;
            e.Handled = true;
            BeginResizeDrag(edge, e);
        };

    private void InitFields()
    {
        string? existing = InstallEngine.ExistingInstall();

        if (_uninstalling)
        {
            _installDir = Program.Args.InstallDir ?? existing ?? AppContext.BaseDirectory;
            TitleText.Text = "Aether Uninstall";
            Heading.Text = "Remove Aether";
            Subheading.Text = $"Aether will be removed from {_installDir}. Your settings can stay behind in case " +
                              "you reinstall later.";
            // The install choices don't apply; reuse the first card's checkbox for the one decision that does.
            DirBox.IsEnabled = false;
            DirBox.Text = _installDir;
            DirNote.Text = "";
            StartMenuCheck.Content = "Keep my settings and saved game data";
            StartMenuCheck.IsChecked = true;
            DesktopCheck.IsVisible = FlashCheck.IsVisible = ProjectorCheck.IsVisible = false;
            AssocNote.Text = "";
            NextButton.Content = "Remove";
            return;
        }

        _options.InstallDir = existing ?? InstallOptions.DefaultInstallDir;
        DirBox.Text = _options.InstallDir;
        StartMenuCheck.IsChecked = _options.StartMenuShortcut;
        DesktopCheck.IsChecked = _options.DesktopShortcut;
        FlashCheck.IsChecked = _options.AssociateFlash;
        ProjectorCheck.IsChecked = _options.AssociateProjector;

        // The launcher does not know what it is about to install until it has asked the releases page, which
        // it does at install time rather than on this screen. Naming its own version here would name the
        // launcher's, which is not the version anyone ends up with.
        bool downloads = InstallEngine.SourceOfFiles == InstallEngine.FileSource.Download;
        string version = SetupInfo.Version;
        string incoming = downloads ? "the latest version" : $"version {version}";

        if (existing is not null && InstallEngine.ExistingVersion() is { } have)
        {
            Heading.Text = "Update Aether";
            Subheading.Text = $"Version {have} is installed. This will replace it with {incoming}. Your settings " +
                              "and saved game data are kept.";
            NextButton.Content = "Update";
        }
        else
        {
            Heading.Text = "Install Aether";
            Subheading.Text = downloads
                ? "An AdventureQuest Worlds client with a GPU renderer. The latest version is downloaded when you install."
                : $"Version {version}. An AdventureQuest Worlds client with a GPU renderer.";
        }

        AssocNote.Text = "Aether is offered in \"Open with\" and listed in Windows' Default apps — it doesn't take " +
                         "your file types over. You choose the default there, whenever you like.";
        ValidateDir();
    }

    private void ValidateDir()
    {
        if (_uninstalling) return;
        _options.InstallDir = DirBox.Text?.Trim() ?? "";
        string? why = _options.Validate();
        DirNote.Text = why ?? "Installs for you only, so it needs no administrator rights.";
        DirNote.Foreground = why is null ? Brush("TextMutedBrush") : Brush("CloseHoverBrush");
        // Not Payload.Exists: the launcher has no payload and downloads instead, and testing for one here
        // would leave its Install button disabled forever with nothing on screen to say why.
        NextButton.IsEnabled = why is null && InstallEngine.SourceOfFiles != InstallEngine.FileSource.None;
    }

    private IBrush Brush(string key) => this.TryFindResource(key, out object? v) && v is IBrush b ? b : Brushes.White;

    // ---- pages -------------------------------------------------------------------------------------------

    /// <summary>Slide the outgoing page out, swap, slide the incoming one in. Direction follows travel, so going
    /// forward and coming back don't feel like the same move.</summary>
    private async Task GoTo(Page page, bool forward = true)
    {
        PageHost.Opacity = 0;
        PageHost.RenderTransform = TransformOperations.Parse(forward ? "translateX(26px)" : "translateX(-26px)");
        await Task.Delay(150);

        _page = page;
        OptionsPage.IsVisible = page == Page.Options;
        ProgressPage.IsVisible = page == Page.Working;
        DonePage.IsVisible = page == Page.Done;

        PageHost.RenderTransform = TransformOperations.Parse(forward ? "translateX(-26px)" : "translateX(26px)");
        await Task.Delay(1);
        PageHost.Opacity = 1;
        PageHost.RenderTransform = TransformOperations.Parse("translateX(0px)");
    }

    private async Task OnPrimary()
    {
        switch (_page)
        {
            case Page.Options:
                await Work();
                break;
            case Page.Done:
                if (!_uninstalling && LaunchCheck.IsChecked == true)
                {
                    string exe = Path.Combine(_options.InstallDir, InstallEngine.ExeName);
                    try { if (File.Exists(exe)) Process.Start(new ProcessStartInfo(exe) { UseShellExecute = true, WorkingDirectory = _options.InstallDir }); }
                    catch { /* nothing useful to do */ }
                }
                Close();
                break;
        }
    }

    private async Task Work()
    {
        _options.InstallDir = DirBox.Text?.Trim() ?? _options.InstallDir;
        _options.StartMenuShortcut = StartMenuCheck.IsChecked == true;
        _options.DesktopShortcut = DesktopCheck.IsChecked == true;
        _options.AssociateFlash = FlashCheck.IsChecked == true;
        _options.AssociateProjector = ProjectorCheck.IsChecked == true;

        await GoTo(Page.Working);
        NextButton.IsEnabled = false;
        BackButton.Content = "Cancel";
        ProgressHeading.Text = _uninstalling ? "Removing Aether" : "Installing Aether";
        _cts = new CancellationTokenSource();

        var progress = new Progress<InstallEngine.Progress>(p => Dispatcher.UIThread.Post(() =>
        {
            Bar.Value = p.Fraction;
            ProgressStage.Text = p.Stage;
            Status.Text = p.Stage;
        }));

        try
        {
            if (_uninstalling)
                await InstallEngine.UninstallAsync(_installDir, StartMenuCheck.IsChecked == true, progress, Log, _cts.Token);
            else
                await InstallEngine.InstallAsync(_options, SetupInfo.Version, progress, Log, _cts.Token);

            await GoTo(Page.Done);
            DoneHeading.Text = _uninstalling ? "Aether has been removed" : "Aether is installed";
            DoneNote.Text = _uninstalling
                ? "Thanks for trying it. You can reinstall any time by running the setup again."
                : $"Installed to {_options.InstallDir}. It's in your Start menu, and offered in \"Open with\" for " +
                  "the file types you picked.";
            LaunchCheck.IsVisible = !_uninstalling;
            LaunchCheck.IsChecked = !_uninstalling;
            NextButton.Content = "Finish";
            NextButton.IsEnabled = true;
            BackButton.IsVisible = false;
            Status.Text = "";
        }
        catch (OperationCanceledException)
        {
            Status.Text = "Cancelled.";
            Log("Cancelled.");
            await GoTo(Page.Options, forward: false);
            RestoreOptionsButtons();
        }
        catch (Exception ex)
        {
            Status.Text = ex.Message.Split('\n')[0];
            Log("ERROR: " + ex);
            LogPanel.IsVisible = true;
            DetailsToggle.Content = "Hide details";
            NextButton.Content = "Try again";
            NextButton.IsEnabled = true;
            BackButton.Content = "Close";
            _page = Page.Options;   // the button retries rather than moving on
        }
        finally
        {
            _cts?.Dispose();
            _cts = null;
        }
    }

    private void RestoreOptionsButtons()
    {
        NextButton.Content = _uninstalling ? "Remove" : "Install";
        NextButton.IsEnabled = true;
        BackButton.Content = "Cancel";
    }

    private async Task PickFolder()
    {
        try
        {
            var dirs = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions
            {
                Title = "Choose where to install Aether", AllowMultiple = false,
            });
            if (dirs.Count > 0 && dirs[0].TryGetLocalPath() is { } path)
            {
                // A picker returns the folder you're IN; installing straight into "Program Files" itself would
                // scatter Aether's files among everyone else's, so add our own folder unless it's already there.
                if (!Path.GetFileName(path.TrimEnd(Path.DirectorySeparatorChar)).Equals("Aether", StringComparison.OrdinalIgnoreCase))
                    path = Path.Combine(path, "Aether");
                DirBox.Text = path;
            }
        }
        catch { /* picker unavailable */ }
    }

    private void Log(string line) => Dispatcher.UIThread.Post(() =>
    {
        _log.AppendLine(line);
        if (_log.Length > 120_000) _log.Remove(0, 60_000);
        LogText.Text = _log.ToString();
        LogScroll.ScrollToEnd();
    });
}
