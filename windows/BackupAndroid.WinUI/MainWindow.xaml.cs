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

    public MainWindow()
    {
        InitializeComponent();
        Title = "Android Backup";
        SystemBackdrop = new MicaBackdrop();
        ActivityList.ItemsSource = _activity;
        DestinationBox.Text = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "AndroidBackups");

        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd));
        var icon = Path.Combine(AppContext.BaseDirectory, "Assets", "icon.ico");
        if (File.Exists(icon)) appWindow.SetIcon(icon);
        appWindow.Resize(new Windows.Graphics.SizeInt32(1180, 800));
        Shell.SelectedItem = Shell.MenuItems[0];
        Log("WinUI 3 ready. Windows administrator permission was requested at startup.");
    }

    private void OnNavigationChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        var tag = (args.SelectedItem as NavigationViewItem)?.Tag?.ToString() ?? "overview";
        OverviewPage.Visibility = tag == "overview" ? Visibility.Visible : Visibility.Collapsed;
        ApplicationsPage.Visibility = tag == "apps" ? Visibility.Visible : Visibility.Collapsed;
        StoragePage.Visibility = tag == "storage" ? Visibility.Visible : Visibility.Collapsed;
        SecurityPage.Visibility = tag == "security" ? Visibility.Visible : Visibility.Collapsed;
        SetupPage.Visibility = tag == "setup" ? Visibility.Visible : Visibility.Collapsed;
    }

    private void Log(string message)
    {
        _activity.Insert(0, $"{DateTime.Now:HH:mm:ss}  {message}");
        while (_activity.Count > 120) _activity.RemoveAt(_activity.Count - 1);
        FooterText.Text = message;
    }

    private void SetBusy(bool busy, bool cancellable = false)
    {
        StartButton.IsEnabled = !busy;
        CancelButton.IsEnabled = busy && cancellable;
        WorkProgress.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
    }

    private string SelectedDevice => DeviceCombo.SelectedItem?.ToString() ?? "";
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
                ApplicationChoices.Items.Clear();
                foreach (var item in data.GetProperty("packages").EnumerateArray())
                    ApplicationChoices.Items.Add(item.GetString() ?? "");
                ApplicationChoices.SelectAll();
                AppCount.Text = $"{ApplicationChoices.Items.Count} apps found";
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
        using var source = new CancellationTokenSource();
        _activeWork = source;
        SetBusy(true, cancellable);
        try
        {
            await _backend.RunAsync(request, HandleEvent, source.Token);
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

        var apps = ApplicationChoices.SelectedItems
            .Cast<string>().Where(x => !string.IsNullOrWhiteSpace(x)).ToArray();
        await RunBackendAsync(new
        {
            action = "backup",
            adb_path = AdbExecutable,
            serial = SelectedDevice,
            destination = DestinationBox.Text,
            packages = apps,
            storage,
            force_stop = StopAppsCheck.IsChecked == true
        }, cancellable: true);
    }

    private void Cancel_Click(object sender, RoutedEventArgs e) => _activeWork?.Cancel();

    private async void Verify_Click(object sender, RoutedEventArgs e)
    {
        var selected = await PickFolderAsync();
        if (selected is null) return;
        await RunBackendAsync(new { action = "verify", folder = selected });
    }

    private void SelectAllApps_Click(object sender, RoutedEventArgs e) => ApplicationChoices.SelectAll();
    private void ClearApps_Click(object sender, RoutedEventArgs e) => ApplicationChoices.SelectedItems.Clear();

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
        await dialog.ShowAsync();
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
        return await dialog.ShowAsync() == ContentDialogResult.Primary;
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
