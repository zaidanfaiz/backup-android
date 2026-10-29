using System.Runtime.InteropServices;

namespace BackupAndroid.WinUI;

internal static class StartupDiagnostics
{
    private static readonly string LogFile = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "AndroidBackup", "Logs", "startup.log");

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int MessageBox(IntPtr hwnd, string text, string caption, uint type);

    internal static void Write(string message)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(LogFile)!);
            File.AppendAllText(LogFile, $"{DateTimeOffset.Now:O} {message}{Environment.NewLine}");
        }
        catch (Exception)
        {
            // Logging must never make application startup fail.
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
