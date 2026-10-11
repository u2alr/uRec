# Plays a 1 kHz tone for N seconds through the default output device (used to test loopback capture).
param([int]$Seconds = 12)
$rate = 48000; $n = $rate * $Seconds
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([Text.Encoding]::ASCII.GetBytes("RIFF")); $bw.Write([int](36 + $n * 2))
$bw.Write([Text.Encoding]::ASCII.GetBytes("WAVEfmt ")); $bw.Write([int]16); $bw.Write([int16]1); $bw.Write([int16]1)
$bw.Write([int]$rate); $bw.Write([int]($rate * 2)); $bw.Write([int16]2); $bw.Write([int16]16)
$bw.Write([Text.Encoding]::ASCII.GetBytes("data")); $bw.Write([int]($n * 2))
for ($i = 0; $i -lt $n; $i++) { $bw.Write([int16](8000 * [math]::Sin(2 * [math]::PI * 1000 * $i / $rate))) }
$bw.Flush(); $ms.Position = 0
$p = New-Object System.Media.SoundPlayer($ms)
$p.PlaySync()
