using System;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using Avalonia.Media;
using Aether.Setup.Views;

namespace Aether.Setup;

public partial class App : Application
{
    public override void Initialize() => AvaloniaXamlLoader.Load(this);

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            // A setup that dies before drawing anything is indistinguishable from one that never started, and
            // this is a WinExe with no console to explain itself. Always put something on screen.
            try { desktop.MainWindow = new SetupWindow(); Program.Note("window constructed"); }
            catch (Exception ex)
            {
                Program.Crash("window", ex);
                desktop.MainWindow = new Window
                {
                    Title = "Aether Setup — failed to start",
                    Width = 720, Height = 420,
                    WindowStartupLocation = WindowStartupLocation.CenterScreen,
                    Background = new SolidColorBrush(Color.FromRgb(0x0A, 0x16, 0x20)),
                    Content = new ScrollViewer
                    {
                        Padding = new Thickness(22),
                        Content = new SelectableTextBlock
                        {
                            Text = ex.ToString(), Foreground = Brushes.White, FontSize = 11.5,
                            TextWrapping = TextWrapping.Wrap,
                        },
                    },
                };
            }
        }
        base.OnFrameworkInitializationCompleted();
    }
}
