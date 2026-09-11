# Toki 설치 스크립트 (Windows 10/11 · x64)
#
#   irm https://toki.dkdk.me/install.ps1 | iex
#
# 먼저 읽고 실행하고 싶다면:
#   irm https://toki.dkdk.me/install.ps1 -OutFile install.ps1
#   notepad install.ps1; powershell -ExecutionPolicy Bypass -File install.ps1
#
# macOS의 install.sh와 같은 규율: 파일명은 릴리스의 windows.txt가 알려주고(버전
# 하드코딩 금지), SHA256SUMS로 검증해 어긋나면 **설치하지 않는다**. 설치는
# 사용자 계정 범위(관리자 권한 불필요). SmartScreen 경고는 코드 서명 인증서가
# 없는 동안 뜬다 — 이 스크립트는 그걸 우회하지 않는다.
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = if ($env:TOKI_REPO) { $env:TOKI_REPO } else { 'Daekyo-Jeong/toki' }
$Base = "https://github.com/$Repo/releases/latest/download"

function Say($m) { Write-Host $m }
function Die($m) { Write-Host "x $m" -ForegroundColor Red; exit 1 }

if (-not [Environment]::Is64BitOperatingSystem) { Die '64비트 Windows 전용이에요.' }

# ── 받을 파일 이름은 릴리스가 알려준다 ─────────────────────────────────
try {
  $Version = ([string](Invoke-RestMethod -Uri "$Base/version.txt" -TimeoutSec 30)).Trim()
  $File    = ([string](Invoke-RestMethod -Uri "$Base/windows.txt" -TimeoutSec 30)).Trim()
} catch { Die '릴리스 정보를 못 읽었어요. 네트워크나 릴리스 상태를 확인해 주세요.' }
if (-not $File) { Die 'Windows 설치본이 아직 릴리스에 없어요.' }
Say "-> Toki $Version 설치 ($File)"

$Tmp = Join-Path $env:TEMP ('toki-install-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Path $Tmp | Out-Null
$Setup = Join-Path $Tmp $File
try {
  Say '-> 받는 중...'
  Invoke-WebRequest -Uri "$Base/$File" -OutFile $Setup -TimeoutSec 600

  # ── 무결성 ─────────────────────────────────────────────────────────
  $Sums = $null
  try { $Sums = [string](Invoke-RestMethod -Uri "$Base/SHA256SUMS" -TimeoutSec 30) } catch {}
  if ($Sums) {
    $Expected = $null
    foreach ($line in ($Sums -split "`n")) {
      $parts = $line.Trim() -split '\s+'
      if ($parts.Length -ge 2 -and $parts[-1].TrimStart('*') -eq $File) { $Expected = $parts[0]; break }
    }
    if ($Expected) {
      $Actual = (Get-FileHash -Path $Setup -Algorithm SHA256).Hash.ToLower()
      if ($Expected.ToLower() -ne $Actual) { Die '체크섬 불일치 — 손상되었거나 변조됐어요. 설치 중단.' }
      Say '  v 체크섬 확인'
    } else {
      Say "  ! SHA256SUMS 에 $File 항목이 없어요 — 검증 건너뜀"
    }
  } else {
    Say '  ! SHA256SUMS 를 못 받았어요 — 검증 건너뜀'
  }

  # ── 설치 (NSIS 무음, 현재 사용자) ───────────────────────────────────
  Say '-> 설치 중...'
  $p = Start-Process -FilePath $Setup -ArgumentList '/S' -Wait -PassThru
  if ($p.ExitCode -ne 0) { Die "설치 프로그램이 실패했어요 (exit $($p.ExitCode))" }

  # NSIS(currentUser)는 %LOCALAPPDATA%\Toki 에 깐다. 실행 파일 이름은 cargo 패키지명
  # (`app.exe`)이다 — 제품명으로 바꾸면 macOS 업데이터 경로까지 같이 바뀌어 보류.
  $Dir = Join-Path $env:LOCALAPPDATA 'Toki'
  $Exe = @('Toki.exe', 'app.exe') | ForEach-Object { Join-Path $Dir $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $Exe) { Die "설치는 끝났는데 $Dir 에 실행 파일이 없어요." }
  Say '-> 실행'
  Start-Process -FilePath $Exe
  Say ''
  Say 'v 설치 완료. 작업표시줄 트레이의 토키 아이콘을 눌러 데스크를 여세요.'
} finally {
  Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
