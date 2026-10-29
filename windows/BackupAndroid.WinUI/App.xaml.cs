using Microsoft.UI.Xaml;

namespace BackupAndroid.WinUI;

public partial class App : Application
{
    private Window? _window;

    public App()
    {
        StartupDiagnostics.Write("WinUI application constructor entered.");
        try
        {
            InitializeComponent();
            UnhandledException += (_, args) =>
            {
                StartupDiagnostics.Report("Unhandled XAML exception", args.Exception);
            };
            AppDomain.CurrentDomain.UnhandledException += (_, args) =>
            {
                if (args.ExceptionObject is Exception exception)
                    StartupDiagnostics.Write($"Unhandled .NET exception: {exception}");
            };
        }
        catch (Exception error)
        {
            StartupDiagnostics.Report("App.InitializeComponent", error);
            throw;
        }
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        try
        {
            StartupDiagnostics.Write("OnLaunched reached.");
            _window = new MainWindow();
            _window.Activate();
            StartupDiagnostics.Write("Main window activated.");
        }
        catch (Exception error)
        {
            StartupDiagnostics.Report("WinUI window initialization", error);
            Exit();
        }
    }
}
