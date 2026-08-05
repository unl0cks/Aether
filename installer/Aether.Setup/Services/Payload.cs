using System;
using System.Formats.Tar;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using System.Threading;
using System.Threading.Tasks;

namespace Aether.Setup.Services;

/// <summary>Aether's files, compressed inside this executable.
///
/// tar + Brotli rather than a .zip: Deflate leaves the installer noticeably fatter, and Brotli at its highest
/// level is meaningfully smaller while staying entirely inside the framework — no extra dependency to ship in a
/// program whose whole job is shipping programs.</summary>
public static class Payload
{
    public const string ResourceName = "Aether.payload";

    /// <summary>False in a bare build (the one that becomes uninstall.exe) — it says so rather than pretending.</summary>
    public static bool Exists => Stream() is not null;

    public static long CompressedBytes
    {
        get { using var s = Stream(); return s?.Length ?? 0; }
    }

    private static Stream? Stream() => Assembly.GetExecutingAssembly().GetManifestResourceStream(ResourceName);

    /// <summary>Unpack into <paramref name="targetDir"/>, reporting 0..1 as it goes.
    ///
    /// The matching pack step lives in build-setup.ps1 rather than here. It used to be a <c>--pack</c> mode on
    /// this executable, which was neater until you remember that app.manifest asks for administrator: the
    /// build then triggered a UAC prompt just to compress a folder, and the elevated packer held the staging
    /// directory open where the script could neither wait for it nor clean up after it. Compressing files
    /// needs no privileges, so it doesn't get any. The script uses the same two framework APIs this does —
    /// <c>TarFile.CreateFromDirectory</c> into a <c>BrotliStream</c>, no base directory — and the Brotli
    /// quality it picks is irrelevant here, since decompression doesn't care what level produced the stream.</summary>
    public static async Task ExtractAsync(string targetDir, IProgress<double>? progress,
                                          Action<string>? log, CancellationToken ct)
    {
        await using var res = Stream() ?? throw new InvalidOperationException(
            "This build carries no payload, so it can't install anything.");
        Directory.CreateDirectory(targetDir);

        // Progress is measured against the COMPRESSED stream position: the uncompressed total isn't known
        // without decompressing twice, and a bar that moves steadily beats a precise one that costs a
        // second pass over a hundred megabytes.
        long total = res.Length;
        var counting = new PositionReportingStream(res, p => progress?.Report(total > 0 ? Math.Clamp((double)p / total, 0, 1) : 0));
        await using var brotli = new BrotliStream(counting, CompressionMode.Decompress);
        await using var reader = new TarReader(brotli);

        int files = 0;
        while (await reader.GetNextEntryAsync(cancellationToken: ct) is { } entry)
        {
            ct.ThrowIfCancellationRequested();
            if (entry.EntryType is not (TarEntryType.RegularFile or TarEntryType.V7RegularFile)) continue;

            string rel = entry.Name.Replace('/', Path.DirectorySeparatorChar);
            string dest = Path.GetFullPath(Path.Combine(targetDir, rel));
            // Refuse anything that resolves outside the target. The archive is ours, but an installer that
            // writes wherever a path tells it to is a hole regardless of who built the file.
            if (!dest.StartsWith(Path.GetFullPath(targetDir), StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException($"Payload entry escapes the install folder: {entry.Name}");

            Directory.CreateDirectory(Path.GetDirectoryName(dest)!);
            await entry.ExtractToFileAsync(dest, overwrite: true, ct);
            files++;
        }
        progress?.Report(1);
        log?.Invoke($"Extracted {files} files to {targetDir}");
    }

    /// <summary>Wraps a stream purely to report how far through it we are.</summary>
    private sealed class PositionReportingStream(Stream inner, Action<long> onRead) : Stream
    {
        private long _read;
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => inner.Length;
        public override long Position { get => _read; set => throw new NotSupportedException(); }
        public override void Flush() { }
        public override int Read(byte[] buffer, int offset, int count)
        {
            int n = inner.Read(buffer, offset, count);
            _read += n; onRead(_read);
            return n;
        }
        public override int Read(Span<byte> buffer)
        {
            int n = inner.Read(buffer);
            _read += n; onRead(_read);
            return n;
        }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
    }
}
