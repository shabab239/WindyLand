# Release build: compile, stamp the exe icon and version info, run the
# self-test, build the installer.
#   .\build.ps1                      -> dist\WindyLandSetup-<version>.exe
#   .\build.ps1 -OutDir $HOME\Desktop
# GitHub Actions (release.yml) builds in two halves so the exe can be signed
# before it goes into the installer:
#   .\build.ps1 -SkipInstaller -NoSelfTest   (exe only)
#   .\build.ps1 -InstallerOnly               (installer from the existing exe)
param(
    [string]$OutDir = "$PSScriptRoot\dist",
    [switch]$SkipInstaller,
    [switch]$NoSelfTest,
    [switch]$InstallerOnly
)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
Write-Host "WindyLand $version"

$exe = "$PSScriptRoot\target\release\windyland.exe"
if (-not $InstallerOnly) {
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

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

# Version info (product name and version, shown in the file's Properties and
# required for code signing): a VS_VERSIONINFO resource built by hand.
Add-Type -TypeDefinition @"
using System; using System.IO; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class VersionStamp {
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern IntPtr BeginUpdateResource(string f, bool del);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool UpdateResource(IntPtr h, IntPtr type, IntPtr name, ushort lang, byte[] data, uint size);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool EndUpdateResource(IntPtr h, bool discard);
  static void Pad(List<byte> b) { while (b.Count % 4 != 0) b.Add(0); }
  // One node: wLength, wValueLength, wType, key, padding, value, padding, children.
  static byte[] Node(string key, byte[] value, int valueLen, bool text, List<byte[]> kids) {
    var b = new List<byte>(new byte[6]);
    b.AddRange(Encoding.Unicode.GetBytes(key + "\0"));
    Pad(b);
    if (value != null) { b.AddRange(value); }
    foreach (var k in kids) { Pad(b); b.AddRange(k); }
    int len = b.Count;
    b[0] = (byte)len; b[1] = (byte)(len >> 8);
    b[2] = (byte)valueLen; b[3] = (byte)(valueLen >> 8);
    b[4] = (byte)(text ? 1 : 0); b[5] = 0;
    return b.ToArray();
  }
  static byte[] Str(string k, string v) { return Node(k, Encoding.Unicode.GetBytes(v + "\0"), v.Length + 1, true, new List<byte[]>()); }
  public static void Apply(string exe, string version, Dictionary<string, string> strings) {
    var p = version.Split('.'); int ma = int.Parse(p[0]), mi = int.Parse(p[1]), pa = p.Length > 2 ? int.Parse(p[2]) : 0;
    var f = new MemoryStream(); var w = new BinaryWriter(f);
    uint ms = (uint)((ma << 16) | mi), ls = (uint)(pa << 16);
    foreach (uint x in new uint[] { 0xFEEF04BD, 0x00010000, ms, ls, ms, ls, 0x3F, 0, 0x00040004, 1, 0, 0, 0 }) w.Write(x);
    var table = new List<byte[]>();
    foreach (var kv in strings) table.Add(Str(kv.Key, kv.Value));
    var sfi = Node("StringFileInfo", null, 0, true, new List<byte[]> { Node("040904b0", null, 0, true, table) });
    var tr = new byte[] { 0x09, 0x04, 0xb0, 0x04 };
    var vfi = Node("VarFileInfo", null, 0, true, new List<byte[]> { Node("Translation", tr, 4, false, new List<byte[]>()) });
    var root = Node("VS_VERSION_INFO", f.ToArray(), 52, false, new List<byte[]> { sfi, vfi });
    IntPtr h = BeginUpdateResource(exe, false);
    if (h == IntPtr.Zero) throw new Exception("BeginUpdateResource " + Marshal.GetLastWin32Error());
    if (!UpdateResource(h, (IntPtr)16, (IntPtr)1, 0x0409, root, (uint)root.Length)) throw new Exception("RT_VERSION " + Marshal.GetLastWin32Error());
    if (!EndUpdateResource(h, false)) throw new Exception("EndUpdateResource " + Marshal.GetLastWin32Error());
  }
}
"@
$strings = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$strings['CompanyName'] = 'Shabab'
$strings['FileDescription'] = 'WindyLand'
$strings['FileVersion'] = $version
$strings['InternalName'] = 'windyland'
$strings['LegalCopyright'] = 'Copyright (C) 2026 Shabab. MIT License.'
$strings['OriginalFilename'] = 'windyland.exe'
$strings['ProductName'] = 'WindyLand'
$strings['ProductVersion'] = $version
[VersionStamp]::Apply($exe, $version, $strings)
$vi = (Get-Item $exe).VersionInfo
if ($vi.ProductName -ne 'WindyLand' -or $vi.ProductVersion -ne $version) { throw "version info not readable" }
Write-Host "Version info embedded ($($vi.ProductName) $($vi.ProductVersion))"

}

# Self-test in a scratch data folder.
if (-not $NoSelfTest -and -not $InstallerOnly) {
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
}

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
