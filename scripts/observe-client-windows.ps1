# Read-only, bounded window inventory. Never closes or activates client windows.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class PilotWeaveWindowInventory {
  delegate bool Callback(IntPtr window, IntPtr parameter);
  [DllImport("user32.dll")] static extern bool EnumWindows(Callback callback, IntPtr parameter);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
  public static string[] Read() {
    var found = new List<string>();
    EnumWindows((window, unused) => {
      if (!IsWindowVisible(window)) return true;
      uint pid; GetWindowThreadProcessId(window, out pid);
      try {
        using (var process = Process.GetProcessById((int)pid)) {
          var name = process.ProcessName.ToLowerInvariant();
          if (name == "code" || name == "code - insiders" || name == "pilotweave")
            found.Add(name + ":" + pid + ":" + window.ToInt64());
        }
      } catch (ArgumentException) {} catch (InvalidOperationException) {}
      return true;
    }, IntPtr.Zero);
    found.Sort(); return found.ToArray();
  }
}
'@
$deadline = [DateTime]::UtcNow.AddMinutes(15)
$previous = $null
while ([DateTime]::UtcNow -lt $deadline) {
  $current = ConvertTo-Json -Compress -InputObject @([PilotWeaveWindowInventory]::Read())
  if ($current -ne $previous) {
    [Console]::Out.WriteLine($current)
    [Console]::Out.Flush()
    $previous = $current
  }
  Start-Sleep -Milliseconds 100
}
