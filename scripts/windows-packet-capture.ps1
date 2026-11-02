# Watches the network card of a Windows runner with pktmon while raw ICMP echo requests are sent to
# an address nobody answers (192.0.2.1, the documentation range), and counts the echo requests that
# appear. Three runs of the test `raw_echo_for_the_packet_capture` (crates/engine/tests/sandbox.rs):
#
#   control          a program that is not locked in: its packets must be seen, or the capture
#                    proves nothing about the others
#   sandbox          the engine's sandbox as built: nothing must be seen
#   sandbox-with-privilege
#                    the sandbox with the privilege the app removes put back (a debug switch of the
#                    client): shows what a raw echo from an AppContainer token that still has it does
#                    on the network card. Recorded only.
#
# Everything goes to logs/ and the summary of the job. The test programs must be built already.
$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force logs | Out-Null
$target = '192.0.2.1'
$log = Join-Path $PWD 'logs/capture.txt'
function Say($text) { $text | Tee-Object -FilePath $log -Append }

$counts = [ordered]@{}
function Capture([string]$phase, [string]$keepPrivilege) {
  Say "=== $phase ==="
  pktmon stop 2>&1 | Out-Null
  pktmon filter remove 2>&1 | Out-Null
  Say (pktmon filter add 'documentation-address' -i $target 2>&1 | Out-String)
  Say (pktmon filter list 2>&1 | Out-String)
  $etl = Join-Path $PWD "logs/$phase.etl"
  Say (pktmon start --capture --pkt-size 0 --file-name $etl 2>&1 | Out-String)
  Start-Sleep -Seconds 3
  $env:PARTRIDGE_CAPTURE_PHASE = if ($phase -eq 'control') { 'control' } else { 'sandbox' }
  $env:PARTRIDGE_CAPTURE_TARGET = $target
  if ($keepPrivilege) { $env:PARTRIDGE_TEST_KEEP_TRAVERSE_PRIVILEGE = '1' }
  $test = cargo test -p partridge-engine --locked --test sandbox raw_echo_for_the_packet_capture -- --ignored --nocapture 2>&1 | Out-String
  Remove-Item Env:PARTRIDGE_TEST_KEEP_TRAVERSE_PRIVILEGE -ErrorAction SilentlyContinue
  Say $test
  Start-Sleep -Seconds 3
  Say (pktmon stop 2>&1 | Out-String)
  $txt = Join-Path $PWD "logs/$phase.txt"
  Say (pktmon etl2txt $etl -o $txt 2>&1 | Out-String)
  $echo = 0
  $any = 0
  if (Test-Path $txt) {
    $all = Get-Content -LiteralPath $txt
    $any = @($all | Where-Object { $_ -match [regex]::Escape($target) }).Count
    $echo = @($all | Where-Object { $_ -match [regex]::Escape($target) -and $_ -match 'ICMP' }).Count
    Say "lines with the address: $any, of which ICMP: $echo"
    $all | Where-Object { $_ -match [regex]::Escape($target) } | Select-Object -First 6 | ForEach-Object { Say "  $_" }
  } else {
    Say 'no text file was made from the capture'
  }
  $script:counts[$phase] = $echo
  $sent = ([regex]::Matches($test, 'raw_echo_sent_\d: ALLOWED')).Count
  $refused = ([regex]::Matches($test, 'raw_echo_sent_\d: denied')).Count
  Say "echo requests the program says it sent: $sent; refused: $refused"
  $script:counts["$phase-sent"] = $sent
}

Capture 'control' $false
Capture 'sandbox' $false
Capture 'sandbox-with-privilege' $true

$summary = @("### Raw echo requests to $target seen by pktmon", '', '| Run | Packets seen | Program says sent |', '|---|---|---|')
foreach ($phase in 'control', 'sandbox', 'sandbox-with-privilege') {
  $summary += "| $phase | $($counts[$phase]) | $($counts["$phase-sent"]) |"
}
$summary | ForEach-Object { Say $_ }
if ($env:GITHUB_STEP_SUMMARY) { $summary | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }

if ($counts['control-sent'] -lt 1) { throw 'The control could not send an echo request: the capture proves nothing.' }
if ($counts['control'] -lt 1) { throw 'pktmon did not see the control''s echo requests: the capture proves nothing about the sandbox.' }
if ($counts['sandbox-sent'] -ne 0) { throw 'The sandbox sent an echo request.' }
if ($counts['sandbox'] -ne 0) { throw 'pktmon saw packets to the documentation address from the sandbox run.' }
Say 'The sandbox sent nothing, and the capture sees what the control sends.'
