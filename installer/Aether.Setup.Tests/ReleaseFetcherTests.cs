using System;
using System.Collections.Generic;
using System.IO;
using System.Net;
using System.Net.Http;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Aether.Setup.Services;
using Xunit;

namespace Aether.Setup.Tests;

/// <summary>The half of the launcher that touches the network.
///
/// <see cref="ReleaseSource"/> is pure text in, answer out, and deliberately does no fetching. That left the
/// fetching itself untested, which is where an installer can do real harm: downloading an executable and
/// running it is only defensible if a file that is not what the release published cannot reach the disk. These
/// drive a fake transport so every one of those paths is exercised without a network.</summary>
public sealed class ReleaseFetcherTests
{
    private const string Body = "the aether portable archive, pretend this is a zip";

    private static string Sha256Of(string text)
    {
        byte[] hash = SHA256.HashData(Encoding.UTF8.GetBytes(text));
        return Convert.ToHexString(hash).ToLowerInvariant();
    }

    private static string ReleaseJson(string version) => $$"""
    {
      "tag_name": "v{{version}}",
      "draft": false,
      "prerelease": false,
      "assets": [
        { "name": "Aether-Portable-{{version}}-win-x64.zip",
          "browser_download_url": "https://example.invalid/portable.zip", "size": 51 },
        { "name": "SHA256SUMS.txt",
          "browser_download_url": "https://example.invalid/sums.txt", "size": 200 }
      ]
    }
    """;

    /// <summary>A transport that answers from a table, and counts what was asked for.</summary>
    private sealed class FakeTransport : HttpMessageHandler
    {
        private readonly Dictionary<string, (HttpStatusCode Status, string Body)> _replies = new(StringComparer.OrdinalIgnoreCase);
        public List<string> Requested { get; } = new();
        public string? UserAgent { get; private set; }

        public FakeTransport Reply(string url, string body, HttpStatusCode status = HttpStatusCode.OK)
        {
            _replies[url] = (status, body);
            return this;
        }

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
        {
            string url = request.RequestUri!.ToString();
            Requested.Add(url);
            UserAgent ??= request.Headers.UserAgent.ToString();

            if (!_replies.TryGetValue(url, out var reply))
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound) { Content = new StringContent("") });

            var content = new StringContent(reply.Body);
            content.Headers.ContentLength = Encoding.UTF8.GetByteCount(reply.Body);
            return Task.FromResult(new HttpResponseMessage(reply.Status) { Content = content });
        }
    }

    private static (ReleaseFetcher Fetcher, FakeTransport Transport) Build(string version = "0.5.11", string? body = null, string? sums = null)
    {
        body ??= Body;
        sums ??= $"{Sha256Of(body)}  Aether-Portable-{version}-win-x64.zip";
        var transport = new FakeTransport()
            .Reply(ReleaseFetcher.LatestReleaseUrl, ReleaseJson(version))
            .Reply("https://example.invalid/portable.zip", body)
            .Reply("https://example.invalid/sums.txt", sums);
        return (new ReleaseFetcher(new HttpClient(transport)), transport);
    }

    private static string TempFile() =>
        Path.Combine(Path.GetTempPath(), "aether-fetch-test-" + Guid.NewGuid().ToString("N"));

    [Fact]
    public async Task The_latest_release_is_read_from_the_releases_api()
    {
        var (fetcher, transport) = Build("0.5.11");

        var release = await fetcher.LatestAsync(CancellationToken.None);

        Assert.NotNull(release);
        Assert.Equal("0.5.11", release!.Version);
        Assert.NotNull(release.ClientArchive);
        Assert.Contains(ReleaseFetcher.LatestReleaseUrl, transport.Requested);
    }

    /// <summary>GitHub rejects an API request with no user agent, so sending one is not decoration.</summary>
    [Fact]
    public async Task Requests_identify_themselves()
    {
        var (fetcher, transport) = Build();

        await fetcher.LatestAsync(CancellationToken.None);

        Assert.Contains("Aether", transport.UserAgent);
    }

    /// <summary>A launcher that cannot reach GitHub still has to start what is already installed, so a failed
    /// lookup is an absent release rather than an exception.</summary>
    [Fact]
    public async Task An_unreachable_api_reports_no_release_rather_than_failing()
    {
        var transport = new FakeTransport();
        var fetcher = new ReleaseFetcher(new HttpClient(transport));

        Assert.Null(await fetcher.LatestAsync(CancellationToken.None));
    }

    [Fact]
    public async Task A_download_that_matches_the_published_hash_is_kept()
    {
        var (fetcher, _) = Build();
        var release = await fetcher.LatestAsync(CancellationToken.None);
        string destination = TempFile();

        try
        {
            await fetcher.DownloadClientAsync(release!, destination, null, CancellationToken.None);
            Assert.Equal(Body, await File.ReadAllTextAsync(destination));
        }
        finally { File.Delete(destination); }
    }

    /// <summary>The reason downloading an executable is acceptable at all.</summary>
    [Fact]
    public async Task A_download_that_does_not_match_the_published_hash_is_refused()
    {
        var (fetcher, _) = Build(sums: new string('0', 64) + "  Aether-Portable-0.5.11-win-x64.zip");
        var release = await fetcher.LatestAsync(CancellationToken.None);
        string destination = TempFile();

        try
        {
            var failure = await Assert.ThrowsAsync<InvalidOperationException>(() =>
                fetcher.DownloadClientAsync(release!, destination, null, CancellationToken.None));

            Assert.Contains("hash", failure.Message, StringComparison.OrdinalIgnoreCase);
            Assert.False(File.Exists(destination), "a file that failed verification must not be left behind");
        }
        finally { File.Delete(destination); }
    }

    /// <summary>An asset nobody published a hash for is untrusted, not assumed fine. Otherwise an attacker who
    /// can drop the checksum file gets the verification skipped rather than the download refused.</summary>
    [Fact]
    public async Task An_asset_with_no_published_hash_is_refused()
    {
        var (fetcher, _) = Build(sums: "deadbeef  something-else-entirely.zip");
        var release = await fetcher.LatestAsync(CancellationToken.None);
        string destination = TempFile();

        try
        {
            var failure = await Assert.ThrowsAsync<InvalidOperationException>(() =>
                fetcher.DownloadClientAsync(release!, destination, null, CancellationToken.None));

            Assert.Contains("no published", failure.Message, StringComparison.OrdinalIgnoreCase);
            Assert.False(File.Exists(destination));
        }
        finally { File.Delete(destination); }
    }

    /// <summary>A release with no checksum file at all is the same case: nothing to verify against.</summary>
    [Fact]
    public async Task A_release_without_a_checksum_file_is_refused()
    {
        var transport = new FakeTransport()
            .Reply(ReleaseFetcher.LatestReleaseUrl, """
            {
              "tag_name": "v0.5.11", "draft": false, "prerelease": false,
              "assets": [ { "name": "Aether-Portable-0.5.11-win-x64.zip",
                            "browser_download_url": "https://example.invalid/portable.zip", "size": 51 } ]
            }
            """)
            .Reply("https://example.invalid/portable.zip", Body);
        var fetcher = new ReleaseFetcher(new HttpClient(transport));
        var release = await fetcher.LatestAsync(CancellationToken.None);
        string destination = TempFile();

        try
        {
            await Assert.ThrowsAsync<InvalidOperationException>(() =>
                fetcher.DownloadClientAsync(release!, destination, null, CancellationToken.None));
            Assert.False(File.Exists(destination));
        }
        finally { File.Delete(destination); }
    }

    [Fact]
    public async Task Progress_runs_from_zero_to_one()
    {
        var (fetcher, _) = Build();
        var release = await fetcher.LatestAsync(CancellationToken.None);
        string destination = TempFile();
        var seen = new List<double>();

        try
        {
            await fetcher.DownloadClientAsync(release!, destination,
                new Progress<double>(seen.Add), CancellationToken.None);

            // Progress is marshalled, so the only claim worth making is about the value that mattered.
            Assert.Contains(1.0, seen);
            Assert.All(seen, value => Assert.InRange(value, 0.0, 1.0));
        }
        finally { File.Delete(destination); }
    }

    /// <summary>Downloading over a leftover from an abandoned attempt has to overwrite it, not append to it or
    /// refuse. The partial file is exactly the state an interrupted install leaves behind.</summary>
    [Fact]
    public async Task A_leftover_from_an_abandoned_attempt_is_replaced()
    {
        var (fetcher, _) = Build();
        var release = await fetcher.LatestAsync(CancellationToken.None);
        string destination = TempFile();
        await File.WriteAllTextAsync(destination, "half a download from last time");

        try
        {
            await fetcher.DownloadClientAsync(release!, destination, null, CancellationToken.None);
            Assert.Equal(Body, await File.ReadAllTextAsync(destination));
        }
        finally { File.Delete(destination); }
    }
}
