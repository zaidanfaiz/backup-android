using System.Runtime.InteropServices;

namespace BackupAndroid.WinUI;

internal static class StartupDiagnostics
{
    private static readonly string LogFile = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "AndroidBackup", "Logs", "startup.log");

    private static readonly string PortableLogFile = Path.GetFullPath(Path.Combine(
        AppContext.BaseDirectory, "..", "Logs", "startup.log"));

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int MessageBox(IntPtr hwnd, string text, string caption, uint type);

    internal static void Write(string message)
    {
        var line = $"{DateTimeOffset.Now:O} {message}{Environment.NewLine}";
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(LogFile)!);
            File.AppendAllText(LogFile, line);
        }
        catch (Exception)
        {
            // Logging must never make application startup fail.
        }

        // For a portable distribution, mirror diagnostics to the Logs folder
        // when it exists and is writable. LOCALAPPDATA remains the primary log.
        try
        {
            if (Directory.Exists(Path.GetDirectoryName(PortableLogFile)))
                File.AppendAllText(PortableLogFile, line);
        }
        catch (Exception)
        {
            // Read-only installations do not prevent startup.
        }
    }

    internal static void Report(string phase, Exception exception)
    {
        Write($"{phase}: {exception}");
        try
        {
            MessageBox(IntPtr.Zero,
                $"Android Backup couldn't start ({phase}).\n\n{exception.Message}\n\nDiagnostic log:\n{LogFile}",
                "Android Backup - Startup error",
                0x10);
        }
        catch (Exception) { }
    }
}
