# Release build: compile, stamp the exe icon, run the self-test, build the installer.
#   .\build.ps1                      -> dist\WindyLandSetup-<version>.exe
#   .\build.ps1 -OutDir $HOME\Desktop
param(
    [string]$OutDir = "$PSScriptRoot\dist",
    [switch]$SkipInstaller
)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
Write-Host "WindyLand $version"

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$exe = "$PSScriptRoot\target\release\windyland.exe"

# Embed assets\windyland.ico as the exe icon (works without windres/rc.exe).
Add-Type -TypeDefinition @"
using System; using System.IO; using System.Runtime.InteropServices;
public static class IconStamp {
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern IntPtr BeginUpdateResource(string f, bool del);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool UpdateResource(IntPtr h, IntPtr type, IntPtr name, ushort lang, byte[] data, uint size);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool EndUpdateResource(IntPtr h, bool discard);
  public static void Apply(string exe, string ico) {
    byte[] b = File.ReadAllBytes(ico);
    int n = BitConverter.ToUInt16(b, 4);
    IntPtr h = BeginUpdateResource(exe, false);
    if (h == IntPtr.Zero) throw new Exception("BeginUpdateResource " + Marshal.GetLastWin32Error());
    var ms = new MemoryStream(); var w = new BinaryWriter(ms);
    w.Write((ushort)0); w.Write((ushort)1); w.Write((ushort)n);
    for (int i = 0; i < n; i++) {
      int e = 6 + i * 16; int size = BitConverter.ToInt32(b, e + 8); int off = BitConverter.ToInt32(b, e + 12);
      byte[] img = new byte[size]; Array.Copy(b, off, img, 0, size);
      if (!UpdateResource(h, (IntPtr)3, (IntPtr)(i + 1), 0, img, (uint)size)) throw new Exception("RT_ICON " + Marshal.GetLastWin32Error());
      w.Write(b, e, 12); w.Write((ushort)(i + 1));
    }
    byte[] g = ms.ToArray();
    if (!UpdateResource(h, (IntPtr)14, (IntPtr)1, 0, g, (uint)g.Length)) throw new Exception("RT_GROUP_ICON " + Marshal.GetLastWin32Error());
    if (!EndUpdateResource(h, false)) throw new Exception("EndUpdateResource " + Marshal.GetLastWin32Error());
  }
}
"@
[IconStamp]::Apply($exe, "$PSScriptRoot\assets\windyland.ico")
Write-Host "Icon embedded"

# Self-test in a scratch data folder.
$tmp = Join-Path $env:TEMP "windyland-selftest"
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $tmp | Out-Null
$env:WINDYLAND_DATA_DIR = $tmp
$env:WINDYLAND_SELFTEST_OUT = "$tmp\report.txt"
Start-Process $exe -ArgumentList '--selftest' -Wait
Remove-Item Env:WINDYLAND_DATA_DIR, Env:WINDYLAND_SELFTEST_OUT
$report = Get-Content "$tmp\report.txt"
$fails = @($report | Where-Object { $_ -like 'FAIL*' })
$oks = @($report | Where-Object { $_ -like 'OK*' })
Write-Host "Self-test: $($oks.Count) OK, $($fails.Count) FAIL"
if ($fails.Count -gt 0) { $fails | Write-Host; throw "self-test failed" }

if ($SkipInstaller) { return }

$iscc = @(
    $env:ISCC,
    "$env:LOCALAPPDATA\Programs\InnoSetup6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found. Install it or set `$env:ISCC." }

New-Item -ItemType Directory $OutDir -Force | Out-Null
& $iscc "/DAppVersion=$version" "/DOutDir=$OutDir" "$PSScriptRoot\installer\windyland.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
Write-Host "Installer: $OutDir\WindyLandSetup-$version.exe"
