# Installs everything Tobi needs: Python (only if missing), its own virtual environment and the
# packages, then saves the settings typed in the setup wizard. Run by WindyLandSetup.exe (no admin needed).
param([Parameter(Mandatory = $true)][string]$AppDir)

$ErrorActionPreference = 'Continue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ProgressPreference = 'SilentlyContinue'
$agent = Join-Path $AppDir 'agent'
$venv = Join-Path $agent 'venv'
$vpy = Join-Path $venv 'Scripts\python.exe'
$PyVersion = '3.12.8'

$script:lines = New-Object System.Collections.Generic.List[string]
function Say($m) { Write-Host $m; $script:lines.Add([string]$m) }

function Run([string]$exe, [string[]]$arguments) {
    & $exe @arguments 2>&1 | ForEach-Object { Write-Host ("  " + $_); $script:lines.Add("  " + [string]$_) }
    return $LASTEXITCODE
}

function Find-Python {
    $cands = @()
    $py = Get-Command py.exe -ErrorAction SilentlyContinue
    if ($py) { $cands += , @($py.Source, '-3') }
    foreach ($n in 'python.exe', 'python3.exe') {
        $c = Get-Command $n -ErrorAction SilentlyContinue
        if ($c -and $c.Source -notlike '*\WindowsApps\*') { $cands += , @($c.Source) }
    }
    foreach ($root in "$env:LOCALAPPDATA\Programs\Python", $env:ProgramW6432, $env:ProgramFiles) {
        if (-not $root) { continue }
        Get-ChildItem (Join-Path $root 'Python3*\python.exe') -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | ForEach-Object { $cands += , @($_.FullName) }
    }
    foreach ($c in $cands) {
        try {
            $exe = $c[0]; $pre = @($c | Select-Object -Skip 1)
            $out = & $exe @pre -c "import sys,venv,ensurepip;print('%d.%d'%sys.version_info[:2])" 2>$null
            if ($LASTEXITCODE -eq 0 -and $out) {
                $v = [version]($out | Select-Object -Last 1)
                if ($v -ge [version]'3.10' -and $v -lt [version]'3.14') { return , $c }
            }
        } catch { }
    }
    return $null
}

function Download-File([string]$url, [string]$file) {
    Remove-Item $file -Force -ErrorAction SilentlyContinue
    $errors = @()
    try {
        Invoke-WebRequest -Uri $url -OutFile $file -UseBasicParsing -TimeoutSec 600
        if ((Test-Path $file) -and (Get-Item $file).Length -gt 5MB) { return }
        $errors += "Invoke-WebRequest: incomplete file"
    } catch { $errors += "Invoke-WebRequest: " + $_.Exception.Message }
    Say ("  download method 1 failed: " + $errors[-1])
    $curl = Join-Path $env:SystemRoot 'System32\curl.exe'
    if (Test-Path $curl) {
        Say "  trying curl.exe..."
        $null = Run $curl @('-L', '--fail', '--silent', '--show-error', '--retry', '3', '-o', $file, $url)
        if ((Test-Path $file) -and (Get-Item $file).Length -gt 5MB) { return }
        $errors += "curl failed"
    }
    try {
        Say "  trying BITS..."
        Start-BitsTransfer -Source $url -Destination $file
        if ((Test-Path $file) -and (Get-Item $file).Length -gt 5MB) { return }
    } catch { $errors += "BITS: " + $_.Exception.Message }
    throw ("Could not download Python from python.org. " + ($errors -join ' | '))
}

function Install-Python {
    $url = "https://www.python.org/ftp/python/$PyVersion/python-$PyVersion-amd64.exe"
    $file = Join-Path $env:TEMP "python-$PyVersion-amd64.exe"
    Say "Downloading Python $PyVersion (about 25 MB)..."
    Download-File $url $file
    $sig = Get-AuthenticodeSignature $file
    $who = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { '(unsigned)' }
    Say ("  signature: " + $sig.Status + " / " + $who)
    # Must be signed by the Python Software Foundation and not tampered with. (Status can read
    # 'UnknownError' on PCs that cannot reach the certificate servers; that is accepted.)
    if ($who -notlike '*Python Software Foundation*' -or @('Valid', 'UnknownError', 'NotTrusted') -notcontains [string]$sig.Status) {
        Remove-Item $file -Force -ErrorAction SilentlyContinue
        throw "The downloaded Python installer failed the signature check ($($sig.Status))."
    }
    $target = Join-Path $agent 'python'
    Say "Installing Python for this user (a minute or two)..."
    $argList = '/quiet InstallAllUsers=0 "TargetDir=' + $target + '" PrependPath=0 Include_launcher=0 InstallLauncherAllUsers=0 Include_test=0 Include_doc=0 Shortcuts=0 AssociateFiles=0'
    $p = Start-Process -FilePath $file -ArgumentList $argList -Wait -PassThru
    Remove-Item $file -Force -ErrorAction SilentlyContinue
    $exe = Join-Path $target 'python.exe'
    if (-not (Test-Path $exe)) { throw "The Python installer finished (exit code $($p.ExitCode)) but python.exe is missing." }
    return , @($exe)
}

try {
    Set-Location $agent
    Remove-Item (Join-Path $agent 'setup_error.txt') -Force -ErrorAction SilentlyContinue
    Say "== Step 1/4: Python"
    $found = Find-Python
    if ($found) { Say ("Found Python: " + ($found -join ' ')) } else { Say "No usable Python (3.10 or newer) found."; $found = Install-Python }
    $pyexe = $found[0]; $pre = @($found | Select-Object -Skip 1)

    Say "== Step 2/4: private environment for the agent"
    if (-not (Test-Path $vpy)) {
        if (Test-Path $venv) { Remove-Item $venv -Recurse -Force -ErrorAction SilentlyContinue }
        $rc = Run $pyexe ($pre + @('-m', 'venv', $venv))
        if ($rc -ne 0 -or -not (Test-Path $vpy)) { throw "Could not create the Python environment." }
    } else { Say "Already there." }

    Say "== Step 3/4: packages (python-docx, playwright, psutil, pillow, openpyxl, pypdf)"
    $null = Run $vpy @('-m', 'pip', 'install', '--disable-pip-version-check', '--quiet', '--upgrade', 'pip')
    $rc = Run $vpy @('-m', 'pip', 'install', '--disable-pip-version-check', '-r', (Join-Path $agent 'requirements.txt'))
    if ($rc -ne 0) {
        Say "Retrying once..."
        $rc = Run $vpy @('-m', 'pip', 'install', '--disable-pip-version-check', '-r', (Join-Path $agent 'requirements.txt'))
    }
    if ($rc -ne 0) { throw "Could not install the packages (is the internet connection on?)." }
    $rc = Run $vpy @('-c', 'import docx, playwright, psutil, PIL; print(1)')
    if ($rc -ne 0) { throw "The packages were installed but cannot be loaded." }
    $rc = Run $vpy @('-c', 'import openpyxl, pypdf; print(1)')
    if ($rc -ne 0) { Say "Note: Excel/PDF reading for the knowledge folder is not available (openpyxl/pypdf missing)." }

    Say "== Step 4/4: saving your settings"
    $null = Run $vpy @('-m', 'windyagent.setup_config')
    $null = Run $vpy @('-m', 'windyagent.setup_config', '--test')
    Say "Creating Tobi's knowledge folder (Documents\Tobi\Knowledge)..."
    $null = Run $vpy @('-m', 'windyagent.launcher', '--knowledge-init')
    Say "Tobi setup finished."
    exit 0
} catch {
    $msg = $_.Exception.Message
    Say ("ERROR: " + $msg)
    $tail = (($script:lines | Select-Object -Last 6) -join ' | ') -replace '[^\x20-\x7E]', '?'
    $full = ($msg + ' [last output: ' + $tail + ']') -replace '[\r\n]+', ' '
    if ($full.Length -gt 900) { $full = $full.Substring(0, 900) }
    try { Set-Content -Path (Join-Path $agent 'setup_error.txt') -Value $full -Encoding ASCII } catch { }
    try { Set-Content -Path (Join-Path $agent 'setup_log.txt') -Value $script:lines -Encoding UTF8 } catch { }
    exit 2
}
