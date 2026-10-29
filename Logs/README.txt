Android Backup diagnostic logs
==============================

On launch, Android Backup attempts to copy startup diagnostics to this
portable Logs directory when the folder is writable.

The authoritative, always-attempted per-user diagnostic location remains:
  %LOCALAPPDATA%\AndroidBackup\Logs\startup.log
  %LOCALAPPDATA%\AndroidBackup\Logs\launcher.log

If this portable directory is read-only, logging automatically continues in
the per-user location. Logs can contain file paths, error details and other
diagnostic metadata; review them before sharing.
