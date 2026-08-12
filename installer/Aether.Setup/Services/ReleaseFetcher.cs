using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Http;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;

namespace Aether.Setup.Services;

/// <summary>The half of the launcher that touches the network.
///
/// <see cref="ReleaseSource"/> decides what a release says; this goes and gets it. The split is deliberate:
/// everything that can quietly be wrong lives there and is testable without a network, and everything here is
/// transport, driven in tests through an injected <see cref="HttpClient"/>.
///
/// One rule governs the whole class. A file that does not match the hash the release published never reaches
/// the disk in a usable state, and an asset with no published hash is refused rather than trusted. Downloading
/// an executable and then running it is only defensible on that basis.</summary>
public sealed class ReleaseFetcher : IDisposable
{
    public const string LatestReleaseUrl = "https://api.github.com/repos/unl0cks/Aether/releases/latest";

    /// <summary>GitHub answers an API request with no user agent with 403, so this is required, not manners.</summary>
    private static readonly string Agent = "Aether-Setup/" + SetupInfo.Version;

    private readonly HttpClient _http;
    private readonly bool _ownsClient;

    public ReleaseFetcher() : this(new HttpClient { Timeout = TimeSpan.FromMinutes(10) }, ownsClient: true) { }

    public ReleaseFetcher(HttpClient http, bool ownsClient = false)
    {
        _http = http;
        _ownsClient = ownsClient;
        if (!_http.DefaultRequestHeaders.UserAgent.TryParseAdd(Agent))
            _http.DefaultRequestHeaders.Add("User-Agent", "Aether-Setup");
    }

    public void Dispose()
    {
        if (_ownsClient) _http.Dispose();
    }

    /// <summary>What the releases page is offering, or null if that could not be established.
    ///
    /// Null covers every failure the same way, on purpose. A launcher that cannot reach GitHub still has to
    /// start the copy already on disk, and there is nothing a player can do differently about a 500 than about
    /// a dropped connection.</summary>
    public async Task<ReleaseSource.Release?> LatestAsync(CancellationToken ct)
    {
        try
        {
            using var response = await _http.GetAsync(LatestReleaseUrl, ct);
            if (!response.IsSuccessStatusCode) return null;
            return ReleaseSource.ParseLatest(await response.Content.ReadAsStringAsync(ct));
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>Download the portable archive to <paramref name="destination"/>, verified against the hash the
    /// release published for it.
    ///
    /// Throws rather than returning a result, because every failure here is one a player has to be told about:
    /// unlike a failed update check, there is no sensible way to carry on installing.</summary>
    public async Task DownloadClientAsync(ReleaseSource.Release release, string destination,
                                          IProgress<double>? progress, CancellationToken ct)
    {
        var archive = release.ClientArchive
            ?? throw new InvalidOperationException($"Release {release.Version} publishes no Windows portable archive.");

        // Fetched before the archive rather than after. A release with nothing to verify against is refused
        // whatever it contains, and finding that out first saves pulling a hundred megabytes to discard.
        var checksums = await ChecksumsAsync(release, ct)
            ?? throw new InvalidOperationException(
                $"Release {release.Version} publishes no {ReleaseSource.ChecksumAssetName}, so there is no published hash to check the download against.");

        if (!checksums.ContainsKey(archive.Name))
            throw new InvalidOperationException(
                $"{archive.Name} has no published hash in {ReleaseSource.ChecksumAssetName}. Refusing to install something the release does not vouch for.");

        string? directory = Path.GetDirectoryName(destination);
        if (!string.IsNullOrEmpty(directory)) Directory.CreateDirectory(directory);

        string actualHash;
        try
        {
            actualHash = await StreamToFileAsync(archive, destination, progress, ct);
        }
        catch
        {
            Delete(destination);
            throw;
        }

        if (!ReleaseSource.ChecksumMatches(checksums, archive.Name, actualHash))
        {
            Delete(destination);
            throw new InvalidOperationException(
                $"{archive.Name} does not match the hash {release.Version} published for it. The download was corrupted or the file is not the one that was released; it has been discarded.");
        }
    }

    /// <summary>The published hashes for a release, or null if it does not publish any.</summary>
    public async Task<IReadOnlyDictionary<string, string>?> ChecksumsAsync(ReleaseSource.Release release, CancellationToken ct)
    {
        if (release.Checksums is not { } asset) return null;

        using var response = await _http.GetAsync(asset.DownloadUrl, ct);
        if (!response.IsSuccessStatusCode) return null;

        var parsed = ReleaseSource.ParseChecksums(await response.Content.ReadAsStringAsync(ct));
        return parsed.Count > 0 ? parsed : null;
    }

    /// <summary>Copy the response body to disk, hashing it on the way past, and report progress.
    ///
    /// Hashed while streaming rather than by reading the file back: a second pass over a hundred megabytes
    /// buys nothing, and the bytes that were written are exactly the bytes that were hashed.
    ///
    /// <c>FileMode.Create</c> truncates, so a leftover from an abandoned attempt is replaced rather than
    /// appended to. An interrupted install leaves exactly that behind.</summary>
    private async Task<string> StreamToFileAsync(ReleaseSource.Asset asset, string destination,
                                                 IProgress<double>? progress, CancellationToken ct)
    {
        using var response = await _http.GetAsync(asset.DownloadUrl, HttpCompletionOption.ResponseHeadersRead, ct);
        response.EnsureSuccessStatusCode();

        // Content-Length where the server gives one, the release's own figure otherwise. Without either there
        // is nothing to be a fraction of, and progress stays at zero until the end rather than inventing a
        // number that would run backwards when it turned out to be wrong.
        long total = response.Content.Headers.ContentLength ?? asset.Size;

        await using var source = await response.Content.ReadAsStreamAsync(ct);
        await using var file = new FileStream(destination, FileMode.Create, FileAccess.Write, FileShare.None,
                                              bufferSize: 128 * 1024, useAsync: true);
        using var hash = SHA256.Create();

        byte[] buffer = new byte[128 * 1024];
        long copied = 0;
        int read;
        while ((read = await source.ReadAsync(buffer, ct)) > 0)
        {
            ct.ThrowIfCancellationRequested();
            await file.WriteAsync(buffer.AsMemory(0, read), ct);
            hash.TransformBlock(buffer, 0, read, null, 0);
            copied += read;
            if (total > 0) progress?.Report(Math.Clamp((double)copied / total, 0, 1));
        }

        hash.TransformFinalBlock([], 0, 0);
        progress?.Report(1);
        return Convert.ToHexString(hash.Hash!).ToLowerInvariant();
    }

    private static void Delete(string path)
    {
        try { File.Delete(path); }
        catch { /* the error being reported matters more than the leftover */ }
    }
}
