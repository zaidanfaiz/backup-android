using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO.Compression;
using System.Net.Http;
using System.Text.Json;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace BackupAndroid.WinUI;

public sealed partial class MainWindow : Window
{
    private const string PlatformToolsZip = "https://dl.google.com/android/repository/platform-tools-latest-windows.zip";
    private const string PlatformToolsPage = "https://developer.android.com/tools/releases/platform-tools";
    private const string OemDriverPage = "https://developer.android.com/studio/run/oem-usb";
    private const string VcRuntimePage = "https://aka.ms/vc14/vc_redist.x64.exe";

    private readonly BackendClient _backend = new();
    private readonly ObservableCollection<string> _activity = new();
    private readonly HttpClient _http = new() { Timeout = TimeSpan.FromMinutes(5) };
    private CancellationTokenSource? _activeWork;
    private bool _rootVerified;
    private bool _restorePlanValidated;
    private bool _restoreTargetMatches;
    private bool _isBusy;
    private bool _updatingAppList;
    private readonly List<string> _availableApps = new();
    private readonly HashSet<string> _selectedApps = new(StringComparer.Ordinal);

    public MainWindow()
    {
        InitializeComponent();
        Title = "Android Backup";
        try
        {
            SystemBackdrop = new MicaBackdrop();
        }
        catch (Exception error)
        {
            StartupDiagnostics.Write($"Mica backdrop unavailable, using normal background: {error.Message}");
        }
        ActivityList.ItemsSource = _activity;
        AdbPathBox.Text = FindInstalledAdb();
        DestinationBox.Text = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "AndroidBackups");

        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd));
        var icon = Path.Combine(AppContext.BaseDirectory, "Assets", "icon.ico");
        try
        {
            if (File.Exists(icon)) appWindow.SetIcon(icon);
            // Fit smaller Windows laptops instead of allowing the command bar
            // to extend beyond the usable screen and trapping scrolling.
            var display = DisplayArea.GetFromWindowId(appWindow.Id, DisplayAreaFallback.Primary);
            var work = display.WorkArea;
            var width = Math.Min(1180, Math.Max(640, work.Width - 64));
            var height = Math.Min(800, Math.Max(480, work.Height - 64));
            appWindow.Resize(new Windows.Graphics.SizeInt32(width, height));
        }
        catch (Exception error)
        {
            StartupDiagnostics.Write($"Optional window decoration unavailable: {error.Message}");
        }
        Shell.SelectedItem = Shell.MenuItems[0];
        Log("WinUI 3 ready. Windows administrator permission was requested at startup.");
    }

    private static string FindInstalledAdb()
    {
        try
        {
            var tools = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "AndroidBackup", "tools");
            if (Directory.Exists(tools))
            {
                var installed = Directory.GetFiles(tools, "adb.exe", SearchOption.AllDirectories)
                    .OrderByDescending(File.GetLastWriteTimeUtc)
                    .FirstOrDefault();
                if (installed is not null) return installed;
            }
        }
        catch (Exception error)
        {
            StartupDiagnostics.Write($"ADB discovery skipped: {error.Message}");
        }
        return "adb";
    }

    private void DeviceSelection_Changed(object sender, SelectionChangedEventArgs e)
    {
        _rootVerified = false;
        _restorePlanValidated = false;
        if (RestoreExecuteButton is not null) RestoreExecuteButton.IsEnabled = false;
        _availableApps.Clear();
        _selectedApps.Clear();
        RebuildAppList();
        if (DeviceInfo is not null) DeviceInfo.Text = "Device selected. Check root to load apps.";
    }

    /// <summary>
    /// Runs in Windows CI only. Exercise each WinUI page, the search list and
    /// enabled scroll viewers without requiring USB or Android root.
    /// </summary>
    internal void RunUiSmokeCheck()
    {
        foreach (var item in Shell.MenuItems.OfType<NavigationViewItem>())
        {
            Shell.SelectedItem = item;
            var tag = item.Tag?.ToString() ?? "";
            var viewer = tag switch
            {
                "overview" => OverviewPage,
                "apps" => ApplicationsPage,
                "storage" => StoragePage,
                "security" => SecurityPage,
                "restore" => RestorePage,
                "setup" => SetupPage,
                _ => throw new InvalidOperationException($"Unknown navigation destination: {tag}")
            };
            if (viewer.Visibility != Visibility.Visible || viewer.VerticalScrollMode != ScrollMode.Enabled)
                throw new InvalidOperationException($"Navigation or vertical scrolling failed on page '{tag}'");
        }

        _availableApps.Clear();
        _selectedApps.Clear();
        _availableApps.AddRange(new[] { "com.example.photos", "org.example.notes", "com.example.music" });
        foreach (var package in _availableApps) _selectedApps.Add(package);
        AppSearchBox.Text = "notes";
        RebuildAppList();
        if (ApplicationChoices.Items.Count != 1 || _selectedApps.Count != 3)
            throw new InvalidOperationException("App search incorrectly discarded backup selections");
        AppSearchBox.Text = "";
        RebuildAppList();
        if (ApplicationChoices.Items.Count != 3)
            throw new InvalidOperationException("App filtering could not be restored");

        // Validate that backup presets actually change the selected components.
        BackupPreset.SelectedIndex = 2;
        if (PrivateDataCheck.IsChecked == true || AppExternalCheck.IsChecked == true)
            throw new InvalidOperationException("APK-only preset unexpectedly includes private app data");
        BackupPreset.SelectedIndex = 0;
        if (ApkCheck.IsChecked != true || PrivateDataCheck.IsChecked != true
            || AppExternalCheck.IsChecked != true || AppObbCheck.IsChecked != true
            || AppMediaCheck.IsChecked != true)
            throw new InvalidOperationException("Complete app backup preset did not enable all components");

        Shell.SelectedItem = Shell.MenuItems[0];
        StartupDiagnostics.Write("UI smoke: navigation, six scroll viewers and app filtering passed.");
        StartupDiagnostics.Write("UI smoke: app data backup presets passed.");
    }

    private void OpenRestore_Click(object sender, RoutedEventArgs e)
    {
        var item = Shell.MenuItems.OfType<NavigationViewItem>()
            .FirstOrDefault(entry => string.Equals(entry.Tag?.ToString(), "restore", StringComparison.Ordinal));
        if (item is not null)
            Shell.SelectedItem = item;
    }

    private void OnNavigationChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        // WinUI can raise SelectionChanged while XAML fields are still being created.
        if (OverviewPage is null || ApplicationsPage is null || StoragePage is null
            || SecurityPage is null || SetupPage is null || RestorePage is null) return;
        var tag = (args.SelectedItem as NavigationViewItem)?.Tag?.ToString() ?? "overview";
        StartupDiagnostics.Write($"UI navigation: {tag}");
        OverviewPage.Visibility = tag == "overview" ? Visibility.Visible : Visibility.Collapsed;
        ApplicationsPage.Visibility = tag == "apps" ? Visibility.Visible : Visibility.Collapsed;
        StoragePage.Visibility = tag == "storage" ? Visibility.Visible : Visibility.Collapsed;
        SecurityPage.Visibility = tag == "security" ? Visibility.Visible : Visibility.Collapsed;
        RestorePage.Visibility = tag == "restore" ? Visibility.Visible : Visibility.Collapsed;
        SetupPage.Visibility = tag == "setup" ? Visibility.Visible : Visibility.Collapsed;
    }

    private void Log(string message)
    {
        StartupDiagnostics.Write($"Activity: {message}");
        _activity.Insert(0, $"{DateTime.Now:HH:mm:ss}  {message}");
        while (_activity.Count > 120) _activity.RemoveAt(_activity.Count - 1);
        FooterText.Text = message;
    }

    private void SetBusy(bool busy, bool cancellable = false)
    {
        _isBusy = busy;
        StartButton.IsEnabled = !busy;
        CancelButton.IsEnabled = busy && cancellable;
        WorkProgress.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        if (RestoreExecuteButton is not null)
            RestoreExecuteButton.IsEnabled = !busy && _rootVerified && _restorePlanValidated && _restoreTargetMatches;
    }

    private string SelectedDevice => DeviceCombo.SelectedItem?.ToString() ?? "";

    private void RebuildAppList()
    {
        if (ApplicationChoices is null || AppCount is null || AppSearchBox is null) return;
        _updatingAppList = true;
        try
        {
            ApplicationChoices.Items.Clear();
            var search = AppSearchBox.Text?.Trim() ?? string.Empty;
            foreach (var package in _availableApps)
            {
                if (package.Contains(search, StringComparison.OrdinalIgnoreCase))
                    ApplicationChoices.Items.Add(package);
            }
            foreach (var package in ApplicationChoices.Items.Cast<string>())
            {
                if (_selectedApps.Contains(package))
                    ApplicationChoices.SelectedItems.Add(package);
            }
            AppCount.Text = $"{_selectedApps.Count} selected / {_availableApps.Count} installed";
        }
        finally
        {
            _updatingAppList = false;
        }
    }

    private void BackupPreset_Changed(object sender, SelectionChangedEventArgs args)
    {
        // XAML can fire SelectionChanged before all controls are initialized.
        if (ApkCheck is null || PrivateDataCheck is null || AppExternalCheck is null
            || AppObbCheck is null || AppMediaCheck is null)
            return;

        switch (BackupPreset.SelectedIndex)
        {
            case 0: // Complete
                ApkCheck.IsChecked = true;
                PrivateDataCheck.IsChecked = true;
                AppExternalCheck.IsChecked = true;
                AppObbCheck.IsChecked = true;
                AppMediaCheck.IsChecked = true;
                break;
            case 1: // App + private data
                ApkCheck.IsChecked = true;
                PrivateDataCheck.IsChecked = true;
                AppExternalCheck.IsChecked = false;
                AppObbCheck.IsChecked = false;
                AppMediaCheck.IsChecked = false;
                break;
            case 2: // APK only
                ApkCheck.IsChecked = true;
                PrivateDataCheck.IsChecked = false;
                AppExternalCheck.IsChecked = false;
                AppObbCheck.IsChecked = false;
                AppMediaCheck.IsChecked = false;
                break;
        }
    }

    private void AppSearch_TextChanged(object sender, TextChangedEventArgs args) => RebuildAppList();

    private void Applications_SelectionChanged(object sender, SelectionChangedEventArgs args)
    {
        if (_updatingAppList) return;
        foreach (var item in args.RemovedItems.OfType<string>())
            _selectedApps.Remove(item);
        foreach (var item in args.AddedItems.OfType<string>())
            _selectedApps.Add(item);
        AppCount.Text = $"{_selectedApps.Count} selected / {_availableApps.Count} installed";
    }
    private string AdbExecutable => string.IsNullOrWhiteSpace(AdbPathBox.Text) ? "adb" : AdbPathBox.Text.Trim();

    private void HandleEvent(JsonElement data)
    {
        var type = data.GetProperty("type").GetString() ?? "";
        switch (type)
        {
            case "devices":
                DeviceCombo.Items.Clear();
                foreach (var item in data.GetProperty("devices").EnumerateArray())
                    DeviceCombo.Items.Add(item.GetString() ?? "");
                if (DeviceCombo.Items.Count > 0) DeviceCombo.SelectedIndex = 0;
                _rootVerified = false;
                DeviceInfo.Text = $"{DeviceCombo.Items.Count} authorized device(s)";
                Log(DeviceInfo.Text);
                break;
            case "device":
                var model = data.GetProperty("model").GetString() ?? "";
                var android = data.GetProperty("android_version").GetString() ?? "";
                var rootProperty = data.GetProperty("root");
                _rootVerified = rootProperty.ValueKind == JsonValueKind.String;
                DeviceInfo.Text = $"{model}  |  Android {android}  |  " +
                    (_rootVerified ? $"Root: {rootProperty.GetString()}" : "Root unavailable");
                _availableApps.Clear();
                _selectedApps.Clear();
                foreach (var item in data.GetProperty("packages").EnumerateArray())
                {
                    var package = item.GetString();
                    if (string.IsNullOrWhiteSpace(package)) continue;
                    _availableApps.Add(package);
                    _selectedApps.Add(package);
                }
                RebuildAppList();
                Log(DeviceInfo.Text);
                break;
            case "progress":
                Log(data.GetProperty("message").GetString() ?? "");
                break;
            case "transferred":
                var file = data.GetProperty("file").GetString() ?? "";
                var bytes = data.GetProperty("bytes").GetInt64();
                FooterText.Text = $"{file}  |  {bytes / 1048576.0:F1} MiB";
                break;
            case "complete":
                Log($"Backup verified and saved: {data.GetProperty("folder").GetString()}");
                break;
            case "verified":
                Log($"SHA-256 verification passed: {data.GetProperty("count").GetInt32()} artifacts.");
                break;
            case "ransomware_scan":
                ShowRansomwareReport(data.GetProperty("report"));
                break;
            case "restore_plan":
                ShowRestorePlan(data.GetProperty("plan"));
                break;
            case "restore_complete":
                var result = data.GetProperty("result");
                RestoreResultText.Text = $"Restored {result.GetProperty("restored_archives").GetInt32()} data archives for {result.GetProperty("package").GetString()}. Safety backup: {result.GetProperty("safety_backup").GetString()}";
                Log(RestoreResultText.Text);
                break;
            case "error":
                Log($"Error: {data.GetProperty("message").GetString()}");
                break;
        }
    }

    private void ShowRansomwareReport(JsonElement report)
    {
        var risk = report.GetProperty("severity").GetString() ?? "none";
        var checkedFiles = report.GetProperty("files_examined").GetInt32();
        var checkedFolders = report.GetProperty("directories_examined").GetInt32();
        var notes = report.GetProperty("ransom_note_matches").GetInt32();
        var extensions = report.GetProperty("suspicious_extension_matches").GetInt32();
        var partial = report.GetProperty("truncated").GetBoolean();

        SecuritySummary.Text = $"Risk indicator: {risk.ToUpperInvariant()}. " +
            (report.GetProperty("message").GetString() ?? "");
        SecurityStatistics.Text = $"{checkedFiles} filenames checked across {checkedFolders} folders" +
            $"  |  {notes} note indicators  |  {extensions} suspicious extensions" +
            (partial ? "  |  Partial scan" : "");

        SecurityFindings.Items.Clear();
        foreach (var item in report.GetProperty("findings").EnumerateArray())
        {
            var category = item.GetProperty("indicator").GetString() ?? "indicator";
            var path = item.GetProperty("path").GetString() ?? "";
            SecurityFindings.Items.Add($"{category}: {path}");
        }

        foreach (var item in report.GetProperty("warnings").EnumerateArray())
        {
            var warning = item.GetString() ?? "";
            SecurityFindings.Items.Add($"Scan note: {warning}");
        }

        if (SecurityFindings.Items.Count == 0)
            SecurityFindings.Items.Add("No matching filenames in scanned folders.");
        Log($"Ransomware indicator scan completed. Risk: {risk}. Files checked: {checkedFiles}.");
    }

    private async void Scan_Click(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrWhiteSpace(SelectedDevice) || !_rootVerified)
        {
            Log("Connect your device and verify active temp root before scanning.");
            return;
        }

        SecuritySummary.Text = "Scanning Android shared storage...";
        SecurityStatistics.Text = "";
        SecurityFindings.Items.Clear();
        await RunBackendAsync(new
        {
            action = "scan_ransomware",
            adb_path = AdbExecutable,
            serial = SelectedDevice
        }, cancellable: true);
    }

    private async Task RunBackendAsync(object request, bool cancellable = false)
    {
        if (_isBusy)
        {
            Log("Another operation is active. Cancel it or finish it before starting another.");
            return;
        }
        using var source = new CancellationTokenSource();
        _activeWork = source;
        SetBusy(true, cancellable);
        try
        {
            // Read the ADB process off the UI thread and marshal all events back
            // through the native WinUI DispatcherQueue. Never update controls
            // directly from the worker thread.
            await Task.Run(() => _backend.RunAsync(request, data =>
            {
                var queued = DispatcherQueue.TryEnqueue(() =>
                {
                    try
                    {
                        HandleEvent(data);
                    }
                    catch (Exception error)
                    {
                        StartupDiagnostics.Report("Handling backup engine event", error);
                    }
                });
                if (!queued)
                    StartupDiagnostics.Write("WinUI event discarded: window dispatcher unavailable");
            }, source.Token), source.Token);
        }
        catch (OperationCanceledException)
        {
            Log("Operation cancelled. A partial backup must not be treated as complete.");
        }
        catch (Exception error)
        {
            Log($"Operation failed: {error.Message}");
            await ShowDialogAsync("Operation failed", error.Message);
        }
        finally
        {
            SetBusy(false);
            _activeWork = null;
        }
    }

    private async void Refresh_Click(object sender, RoutedEventArgs e)
    {
        _rootVerified = false;
        _availableApps.Clear();
        _selectedApps.Clear();
        RebuildAppList();
        await RunBackendAsync(new { action = "devices", adb_path = AdbExecutable });
    }

    private async void Inspect_Click(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrWhiteSpace(SelectedDevice))
        {
            Log("Select an authorized device first.");
            return;
        }
        await RunBackendAsync(new { action = "inspect", adb_path = AdbExecutable, serial = SelectedDevice });
    }

    private async void Backup_Click(object sender, RoutedEventArgs e)
    {
        StartupDiagnostics.Write($"Backup request: selected apps={_selectedApps.Count}; root verified={_rootVerified}");
        if (!_rootVerified || string.IsNullOrWhiteSpace(SelectedDevice))
        {
            Log("Backup blocked. Connect and verify an active root shell first.");
            return;
        }
        if (string.IsNullOrWhiteSpace(DestinationBox.Text))
        {
            Log("Choose a local destination directory.");
            return;
        }
        var storage = new List<string>();
        if (DcimCheck.IsChecked == true) storage.Add("DCIM");
        if (PicturesCheck.IsChecked == true) storage.Add("Pictures");
        if (MoviesCheck.IsChecked == true) storage.Add("Movies");
        if (DownloadsCheck.IsChecked == true) storage.Add("Download");
        if (DocumentsCheck.IsChecked == true) storage.Add("Documents");
        if (MusicCheck.IsChecked == true) storage.Add("Music");
        if (ExternalDataCheck.IsChecked == true) storage.Add("Android/data");
        if (ExternalMediaCheck.IsChecked == true) storage.Add("Android/media");
        if (ObbCheck.IsChecked == true) storage.Add("Android/obb");

        var apps = _selectedApps.OrderBy(x => x, StringComparer.Ordinal).ToArray();
        if (apps.Length == 0 && storage.Count == 0)
        {
            Log("Choose one app or media folder to back up.");
            return;
        }
        await RunBackendAsync(new
        {
            action = "backup",
            adb_path = AdbExecutable,
            serial = SelectedDevice,
            destination = DestinationBox.Text,
            packages = apps,
            storage,
            backup_apk = ApkCheck.IsChecked == true,
            backup_private_data = PrivateDataCheck.IsChecked == true,
            backup_external_data = AppExternalCheck.IsChecked == true,
            backup_obb = AppObbCheck.IsChecked == true,
            backup_app_media = AppMediaCheck.IsChecked == true,
            force_stop = StopAppsCheck.IsChecked == true
        }, cancellable: true);
    }

    private void Cancel_Click(object sender, RoutedEventArgs e) => _activeWork?.Cancel();

    private void ShowRestorePlan(JsonElement plan)
    {
        RestorePackageCombo.Items.Clear();
        foreach (var item in plan.GetProperty("data_packages").EnumerateArray())
        {
            var package = item.GetString();
            if (!string.IsNullOrWhiteSpace(package))
                RestorePackageCombo.Items.Add(package);
        }

        if (RestorePackageCombo.Items.Count > 0)
            RestorePackageCombo.SelectedIndex = 0;

        _restoreTargetMatches = plan.GetProperty("target_matches").GetBoolean();
        _restorePlanValidated = RestorePackageCombo.Items.Count > 0;
        var count = plan.GetProperty("artifacts_verified").GetInt32();
        var original = plan.GetProperty("original_device").GetString() ?? "unknown device";
        var version = plan.GetProperty("android_version").GetString() ?? "?";

        RestorePreviewText.Text = _restoreTargetMatches
            ? $"Verified {count} files. Source: {original}, Android {version}. " +
              $"{RestorePackageCombo.Items.Count} apps contain restorable data. " +
              "A mandatory safety backup will be created before any overwrite."
            : $"Verified {count} files, but device identity or Android version differs. " +
              $"Source: {original}, Android {version}. Restore is blocked.";
        RestoreResultText.Text = "Review the backup and choose an app before restoring.";
        RestoreExecuteButton.IsEnabled = _restoreTargetMatches && _restorePlanValidated && _rootVerified && !_isBusy;
        Log(RestorePreviewText.Text);
    }

    private async void RestoreBrowse_Click(object sender, RoutedEventArgs e)
    {
        if (_isBusy) return;
        var folder = await PickFolderAsync();
        if (folder is null) return;
        RestoreFolderBox.Text = folder;
        RestorePreviewText.Text = "Backup selected. Check integrity and compatibility.";
        RestorePackageCombo.Items.Clear();
        _restorePlanValidated = false;
        _restoreTargetMatches = false;
        RestoreExecuteButton.IsEnabled = false;
    }

    private async void RestorePlan_Click(object sender, RoutedEventArgs e)
    {
        if (_isBusy) return;
        if (string.IsNullOrWhiteSpace(RestoreFolderBox.Text))
        {
            Log("Choose a backup session folder containing manifest.json.");
            return;
        }
        if (string.IsNullOrWhiteSpace(SelectedDevice))
        {
            Log("Connect and select your Android device before checking restore compatibility.");
            return;
        }
        _restorePlanValidated = false;
        RestoreExecuteButton.IsEnabled = false;
        await RunBackendAsync(new
        {
            action = "restore_plan",
            adb_path = AdbExecutable,
            serial = SelectedDevice,
            folder = RestoreFolderBox.Text
        });
    }

    private async void RestoreExecute_Click(object sender, RoutedEventArgs e)
    {
        if (_isBusy || !_rootVerified || !_restorePlanValidated || !_restoreTargetMatches ||
            RestorePackageCombo.SelectedItem is not string package)
        {
            Log("Restore blocked. Validate the backup and verify root on the original device.");
            return;
        }
        if (!await ConfirmAsync("Restore private app data",
            $"Restore data into the installed app {package}?\n\n" +
            "This can overwrite existing app files. Android Keystore-backed " +
            "secrets may not work after restore. A verified safety backup of " +
            "the current app will be created first. Continue?"))
            return;

        var safety = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments),
            "AndroidBackups", "restore-safety");
        RestoreResultText.Text = "Creating safety backup and restoring verified archives...";
        await RunBackendAsync(new
        {
            action = "restore_app",
            adb_path = AdbExecutable,
            serial = SelectedDevice,
            folder = RestoreFolderBox.Text,
            package,
            safety_destination = safety,
            confirm = true
        }, cancellable: true);
    }

    private async void Verify_Click(object sender, RoutedEventArgs e)
    {
        var selected = await PickFolderAsync();
        if (selected is null) return;
        await RunBackendAsync(new { action = "verify", folder = selected });
    }

    private void SelectAllApps_Click(object sender, RoutedEventArgs e)
    {
        if (_isBusy) return;
        foreach (var app in _availableApps) _selectedApps.Add(app);
        RebuildAppList();
    }

    private void ClearApps_Click(object sender, RoutedEventArgs e)
    {
        if (_isBusy) return;
        _selectedApps.Clear();
        RebuildAppList();
    }

    private async void Destination_Click(object sender, RoutedEventArgs e)
    {
        var path = await PickFolderAsync();
        if (path is not null) DestinationBox.Text = path;
    }

    private async Task<string?> PickFolderAsync()
    {
        var picker = new FolderPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        picker.FileTypeFilter.Add("*");
        var folder = await picker.PickSingleFolderAsync();
        return folder?.Path;
    }

    private async Task<string?> PickInfAsync()
    {
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        picker.FileTypeFilter.Add(".inf");
        var file = await picker.PickSingleFileAsync();
        return file?.Path;
    }

    private async Task ShowDialogAsync(string title, string body)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = Content.XamlRoot,
            Title = title,
            Content = body,
            CloseButtonText = "OK",
            DefaultButton = ContentDialogButton.Close
        };
        try
        {
            await dialog.ShowAsync();
        }
        catch (Exception error)
        {
            StartupDiagnostics.Write($"Could not display informational dialog: {error}");
        }
    }

    private async Task<bool> ConfirmAsync(string title, string body)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = Content.XamlRoot,
            Title = title,
            Content = body,
            PrimaryButtonText = "Continue",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close
        };
        try
        {
            return await dialog.ShowAsync() == ContentDialogResult.Primary;
        }
        catch (Exception error)
        {
            StartupDiagnostics.Write($"Could not display confirmation dialog: {error}");
            return false;
        }
    }

    private static void OpenOfficialPage(string url)
    {
        Process.Start(new ProcessStartInfo(url) { UseShellExecute = true });
    }

    private void OpenGoogle_Click(object sender, RoutedEventArgs e) => OpenOfficialPage(PlatformToolsPage);
    private void OpenOem_Click(object sender, RoutedEventArgs e) => OpenOfficialPage(OemDriverPage);
    private void OpenVc_Click(object sender, RoutedEventArgs e) => OpenOfficialPage(VcRuntimePage);

    private async void InstallAdb_Click(object sender, RoutedEventArgs e)
    {
        if (!await ConfirmAsync(
            "Install Android Platform Tools?",
            "Download the official Google Platform Tools ZIP and extract ADB to your local application-data folder?"))
            return;

        SetBusy(true);
        try
        {
            var baseFolder = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "AndroidBackup", "tools");
            Directory.CreateDirectory(baseFolder);

            var stage = Path.Combine(baseFolder, "platform-tools-" + Guid.NewGuid().ToString("N"));
            var archiveFile = Path.Combine(baseFolder, "download-" + Guid.NewGuid().ToString("N") + ".zip");
            try
            {
                using var response = await _http.GetAsync(PlatformToolsZip, HttpCompletionOption.ResponseHeadersRead);
                response.EnsureSuccessStatusCode();
                await using (var output = File.Create(archiveFile))
                    await response.Content.CopyToAsync(output);

                Directory.CreateDirectory(stage);
                using var archive = ZipFile.OpenRead(archiveFile);
                if (archive.Entries.Count > 4000) throw new InvalidDataException("Unexpected ZIP entry count.");
                long total = 0;
                foreach (var entry in archive.Entries)
                {
                    var normalized = entry.FullName.Replace('\\', '/');
                    if (!normalized.StartsWith("platform-tools/", StringComparison.Ordinal)
                        || normalized.Contains("../", StringComparison.Ordinal)
                        || normalized.StartsWith("/", StringComparison.Ordinal))
                        throw new InvalidDataException("Unsafe Platform Tools ZIP entry.");
                    total = checked(total + entry.Length);
                    if (total > 400L * 1024 * 1024)
                        throw new InvalidDataException("Platform Tools exceeds the allowed extracted size.");
                    var relative = normalized["platform-tools/".Length..];
                    if (relative.Length == 0 || relative.EndsWith('/')) continue;
                    var target = Path.GetFullPath(Path.Combine(stage, relative));
                    if (!target.StartsWith(stage + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
                        throw new InvalidDataException("ZIP path escapes destination.");
                    Directory.CreateDirectory(Path.GetDirectoryName(target)!);
                    entry.ExtractToFile(target, overwrite: false);
                }
                var adb = Path.Combine(stage, "adb.exe");
                if (!File.Exists(adb)) throw new InvalidDataException("No adb.exe found in Google's ZIP.");
                AdbPathBox.Text = adb;
                Log($"ADB installed: {adb}");
            }
            catch
            {
                if (Directory.Exists(stage)) Directory.Delete(stage, recursive: true);
                throw;
            }
            finally
            {
                if (File.Exists(archiveFile)) File.Delete(archiveFile);
            }
        }
        catch (Exception error)
        {
            Log($"ADB installation failed: {error.Message}");
            await ShowDialogAsync("ADB installation failed", error.Message);
        }
        finally
        {
            SetBusy(false);
        }
    }

    private async void InstallInf_Click(object sender, RoutedEventArgs e)
    {
        var inf = await PickInfAsync();
        if (inf is null) return;
        if (!await ConfirmAsync(
            "Install USB driver?",
            $"Install this driver using Windows PnPUtil?\n\n{inf}"))
            return;

        await RunWindowsToolAsync("pnputil.exe", "/add-driver", inf, "/install");
    }

    private async void ScanDevices_Click(object sender, RoutedEventArgs e)
        => await RunWindowsToolAsync("pnputil.exe", "/scan-devices");

    private async Task RunWindowsToolAsync(string binary, params string[] arguments)
    {
        SetBusy(true);
        try
        {
            var start = new ProcessStartInfo(binary)
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true
            };
            foreach (var argument in arguments) start.ArgumentList.Add(argument);

            using var process = Process.Start(start) ?? throw new InvalidOperationException("Unable to start Windows driver utility.");
            var stdout = process.StandardOutput.ReadToEndAsync();
            var stderr = process.StandardError.ReadToEndAsync();
            await process.WaitForExitAsync();

            var output = (await stdout).Trim();
            var errors = (await stderr).Trim();
            if (process.ExitCode != 0)
                throw new InvalidOperationException($"Windows driver utility failed (code {process.ExitCode}). {output} {errors}");

            Log(string.IsNullOrEmpty(output) ? "Windows driver operation finished." : output);
        }
        catch (Exception error)
        {
            Log($"Driver operation failed: {error.Message}");
            await ShowDialogAsync("Driver operation failed", error.Message);
        }
        finally
        {
            SetBusy(false);
        }
    }
}
