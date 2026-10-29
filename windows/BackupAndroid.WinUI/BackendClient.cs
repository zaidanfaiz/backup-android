using System.Diagnostics;
using System.Text.Json;

namespace BackupAndroid.WinUI;

/// <summary>
/// Native WinUI 3 frontend communicates with the Rust engine over JSON Lines.
/// All child processes use redirected pipes, never a command-shell window.
/// </summary>
public sealed class BackendClient
{
    private readonly string _binary = Path.Combine(
        AppContext.BaseDirectory, "backend", "backup-android-core.exe");

    public async Task RunAsync(object request, Action<JsonElement> onEvent, CancellationToken token = default)
    {
        if (!File.Exists(_binary))
            throw new FileNotFoundException("Rust engine was not packaged with this application.", _binary);

        using var process = new Process
        {
            StartInfo = new ProcessStartInfo
            {
                FileName = _binary,
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardInput = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                WorkingDirectory = AppContext.BaseDirectory
            }
        };

        process.Start();
        using var registration = token.Register(() =>
        {
            try
            {
                if (!process.HasExited) process.Kill(entireProcessTree: true);
            }
            catch (InvalidOperationException) { }
        });
        var stderrTask = process.StandardError.ReadToEndAsync();

        await process.StandardInput.WriteLineAsync(JsonSerializer.Serialize(request));
        process.StandardInput.Close();

        while (true)
        {
            var line = await process.StandardOutput.ReadLineAsync(token);
            if (line is null) break;
            using var message = JsonDocument.Parse(line);
            onEvent(message.RootElement.Clone());
        }

        await process.WaitForExitAsync(token);
        var stderr = await stderrTask;
        if (process.ExitCode != 0)
            throw new InvalidOperationException(
                string.IsNullOrWhiteSpace(stderr)
                    ? "The Rust engine reported an error. See the activity log."
                    : stderr.Trim());
    }
}
