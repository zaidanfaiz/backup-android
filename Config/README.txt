Android Backup configuration
============================

Do not move AndroidBackup.runtimeconfig.json or *.deps.json from App/.
Those files belong next to the internal WinUI executable and are used by
the .NET runtime on startup.

This Config folder is reserved for app-specific configuration as the
application evolves. Existing platform configuration is kept under App/
to avoid breaking the WinUI bootstrap.
