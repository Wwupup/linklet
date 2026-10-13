# Converts captured bytes only. The caller supplies numeric page and budget values.
$ErrorActionPreference = 'Stop'
$inputBytes = New-Object IO.MemoryStream
[Console]::OpenStandardInput().CopyTo($inputBytes)
$bytes = $inputBytes.ToArray()
$replacement = New-Object Text.DecoderReplacementFallback([string][char]0xfffd)
$encoding = [Text.Encoding]::GetEncoding($page, [Text.EncoderFallback]::ExceptionFallback, $replacement)
$strict = [Text.Encoding]::GetEncoding($page, [Text.EncoderFallback]::ExceptionFallback, [Text.DecoderFallback]::ExceptionFallback)
function Read-Prefix([int]$length) {
    $decoder = $encoding.GetDecoder()
    $chars = New-Object char[] ($encoding.GetMaxCharCount($length))
    $count = $decoder.GetChars($bytes, 0, $length, $chars, 0, $false)
    # A strict flush reports exactly the buffered, unfinished source bytes.
    # They have produced no text and must remain before the next cursor.
    $decoder.Fallback = [Text.DecoderFallback]::ExceptionFallback
    $pending = 0
    try {
        [void]$decoder.GetChars([byte[]]@(), 0, 0, (New-Object char[] 16), 0, $true)
    } catch {
        $failure = $_.Exception
        if ($failure.InnerException) { $failure = $failure.InnerException }
        if ($failure -isnot [Text.DecoderFallbackException]) { throw }
        $pending = $failure.BytesUnknown.Length
    }
    $consumed = $length - $pending
    $lossy = $false
    try { [void]$strict.GetCharCount($bytes, 0, $consumed) } catch {
        $failure = $_.Exception
        if ($failure.InnerException) { $failure = $failure.InnerException }
        if ($failure -isnot [Text.DecoderFallbackException]) { throw }
        $lossy = $true
    }
    @{text=(New-Object string($chars, 0, $count)); consumed=$consumed; pending=$pending; lossy=$lossy}
}
$full = Read-Prefix $bytes.Length
$result = $full
if ([Text.Encoding]::UTF8.GetByteCount($full.text) -gt $budget) {
    $low = 0
    $high = $full.consumed
    $result = Read-Prefix 0
    while ($low -le $high) {
        $middle = $low + [int][Math]::Floor(($high - $low) / 2)
        $value = Read-Prefix $middle
        if ([Text.Encoding]::UTF8.GetByteCount($value.text) -le $budget) {
            $result = $value
            $low = $middle + 1
        } else { $high = $middle - 1 }
    }
    $result.pending = $full.pending
}
[Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
$result | ConvertTo-Json -Compress
