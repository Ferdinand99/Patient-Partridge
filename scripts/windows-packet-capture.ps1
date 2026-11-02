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
function Capture([string]$phase, [bool]$keepPrivilege) {
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
  $dropped = 0
  if (Test-Path $txt) {
    # pktmon writes UTF-16 text. A packet that the stack dropped is shown as a "Drop:" event with
    # the reason; a packet that went on is shown at each component it passed.
    $all = Get-Content -LiteralPath $txt
    $requests = @($all | Where-Object { $_ -match [regex]::Escape($target) -and $_ -match 'ICMP echo request' })
    $dropped = @($requests | Where-Object { $_ -match '^\s*Drop:' }).Count
    $echo = $requests.Count - $dropped
    $reasons = @($all | Where-Object { $_ -match 'Drop: PktGroupId' } | ForEach-Object { if ($_ -match 'DropReason ([^,]+),') { $Matches[1] } } | Sort-Object -Unique)
    Say "echo requests to $target in the capture: $($requests.Count), dropped in the stack: $dropped ($($reasons -join '; ')), passed on: $echo"
    $requests | Select-Object -First 6 | ForEach-Object { Say "  $_" }
  } else {
    Say 'no text file was made from the capture'
  }
  $script:counts[$phase] = $echo
  $script:counts["$phase-dropped"] = $dropped
  $sent = ([regex]::Matches($test, 'raw_echo_sent_\d: ALLOWED')).Count
  $refused = ([regex]::Matches($test, 'raw_echo_sent_\d: denied')).Count
  Say "echo requests the program says it sent: $sent; refused: $refused"
  $script:counts["$phase-sent"] = $sent
  $script:counts["$phase-refused"] = $refused
}

Capture 'control' $false
Capture 'sandbox' $false
Capture 'sandbox-with-privilege' $true

$summary = @("### Raw echo requests to $target seen by pktmon", '', '| Run | Program says sent | Passed on in the stack | Dropped in the stack |', '|---|---|---|---|')
foreach ($phase in 'control', 'sandbox', 'sandbox-with-privilege') {
  $summary += "| $phase | $($counts["$phase-sent"]) | $($counts[$phase]) | $($counts["$phase-dropped"]) |"
}
$summary | ForEach-Object { Say $_ }
if ($env:GITHUB_STEP_SUMMARY) { $summary | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }

if ($counts['control-sent'] -lt 1) { throw 'The control could not send an echo request: the capture proves nothing.' }
if (($counts['control'] + $counts['control-dropped']) -lt 1) { throw 'pktmon did not see the control''s echo requests: the capture proves nothing about the sandbox.' }
if ($counts['sandbox-sent'] -ne 0) { throw 'The sandbox sent an echo request.' }
if ($counts['sandbox-refused'] -ne 5) { throw "The sandbox run did not refuse the 5 raw sockets (refused: $($counts['sandbox-refused'])): it may not have run." }
if (($counts['sandbox'] + $counts['sandbox-dropped']) -ne 0) { throw 'pktmon saw echo requests to the documentation address from the sandbox run.' }
Say 'The sandbox sent nothing, and the capture sees what the control sends.'
