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
            // WinUI initializes application resources during its startup lifecycle.
            // Reading Application.Resources from the App constructor can throw
            // COMException (0x8000FFFF) before the framework is ready.
            // App.xaml declares XamlControlsResources; CI checks its structure.
            InitializeComponent();
            StartupDiagnostics.Write("App.InitializeComponent returned successfully.");

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

            if (Environment.GetCommandLineArgs().Contains("--ui-smoke", StringComparer.Ordinal))
            {
                StartupDiagnostics.Write("Running interactive WinUI smoke test.");
                if (_window is MainWindow mainWindow)
                    mainWindow.RunUiSmokeCheck();
                StartupDiagnostics.Write("UI smoke complete.");
                Exit();
            }
        }
        catch (Exception error)
        {
            StartupDiagnostics.Report("WinUI window initialization", error);
            Exit();
        }
    }
}
