using Aether.Setup.Services;
using Xunit;

namespace Aether.Setup.Tests;

public class ReleaseSourceTests
{
    private const string LatestJson = """
    {
      "tag_name": "v0.5.2",
      "draft": false,
      "prerelease": false,
      "assets": [
        { "name": "Aether-Setup-0.5.2-win-x64.exe", "browser_download_url": "https://example/setup.exe", "size": 106739514 },
        { "name": "Aether-Portable-0.5.2-win-x64.zip", "browser_download_url": "https://example/portable.zip", "size": 11100000 },
        { "name": "SHA256SUMS.txt", "browser_download_url": "https://example/SHA256SUMS.txt", "size": 201 }
      ]
    }
    """;

    [Fact]
    public void The_release_is_read_with_its_version_stripped_of_the_tag_prefix()
    {
        var release = ReleaseSource.ParseLatest(LatestJson);

        Assert.NotNull(release);
        Assert.Equal("0.5.2", release!.Version);
    }

    /// The launcher wants the client archive, not the 100 MB installer sitting next to it.
    [Fact]
    public void The_client_archive_and_checksums_are_picked_out_of_the_assets()
    {
        var release = ReleaseSource.ParseLatest(LatestJson)!;

        Assert.Equal("Aether-Portable-0.5.2-win-x64.zip", release.ClientArchive?.Name);
        Assert.Equal("SHA256SUMS.txt", release.Checksums?.Name);
    }

    /// Version ordering compared as text puts 0.5.10 below 0.5.9, which would strand every player on the
    /// older build from the tenth patch onwards while the update check appeared to run normally.
    [Theory]
    [InlineData("0.5.10", "0.5.9", true)]
    [InlineData("0.5.9", "0.5.10", false)]
    [InlineData("0.6.0", "0.5.99", true)]
    [InlineData("1.0.0", "0.99.99", true)]
    [InlineData("0.5.2", "0.5.2", false)]
    [InlineData("v0.5.3", "0.5.2", true)]
    [InlineData("0.5.2", "v0.5.3", false)]
    [InlineData("0.5.2.1", "0.5.2", true)]
    [InlineData("0.5.2", "0.5.2.1", false)]
    public void Versions_are_ordered_as_numbers_not_as_text(string candidate, string installed, bool expected)
    {
        Assert.Equal(expected, ReleaseSource.IsNewer(candidate, installed));
    }

    /// Nothing installed yet still counts as "there is something newer", or a fresh machine would never
    /// download anything.
    [Fact]
    public void Anything_is_newer_than_nothing_installed()
    {
        Assert.True(ReleaseSource.IsNewer("0.5.2", ""));
        Assert.False(ReleaseSource.IsNewer("", "0.5.2"));
    }

    /// A launcher that throws on a malformed API response cannot start the copy already on disk, which is a
    /// worse failure than not knowing about an update.
    [Theory]
    [InlineData("not json at all")]
    [InlineData("[]")]
    [InlineData("{}")]
    [InlineData("""{ "tag_name": "" }""")]
    public void An_unreadable_response_is_no_release_rather_than_an_exception(string json)
    {
        Assert.Null(ReleaseSource.ParseLatest(json));
    }

    [Theory]
    [InlineData("""{ "tag_name": "v9.9.9", "draft": true, "assets": [] }""")]
    [InlineData("""{ "tag_name": "v9.9.9", "prerelease": true, "assets": [] }""")]
    public void Drafts_and_prereleases_are_not_offered_to_players(string json)
    {
        Assert.Null(ReleaseSource.ParseLatest(json));
    }

    [Fact]
    public void Checksums_are_read_from_the_published_sha256sums_file()
    {
        // Copied verbatim from a real SHA256SUMS.txt, two spaces and all.
        const string text =
            "2e14fad68a1dad9ef8122c652e728f968389f06dd980b381749a99da7ebd716c  Aether-Setup-0.5.2-win-x64.exe\n" +
            "f6955b9b86a6edbcb6aad8de144d8d8d94e6650a0e8d24f5dcaac02a6caa4755  Aether-Portable-0.5.2-win-x64.zip\n";

        var checksums = ReleaseSource.ParseChecksums(text);

        Assert.Equal(2, checksums.Count);
        Assert.True(ReleaseSource.ChecksumMatches(
            checksums,
            "Aether-Portable-0.5.2-win-x64.zip",
            "f6955b9b86a6edbcb6aad8de144d8d8d94e6650a0e8d24f5dcaac02a6caa4755"));
    }

    /// The whole point of the checksum is to refuse a file that is not the one that was published.
    [Fact]
    public void A_hash_that_does_not_match_is_rejected()
    {
        var checksums = ReleaseSource.ParseChecksums(
            "f6955b9b86a6edbcb6aad8de144d8d8d94e6650a0e8d24f5dcaac02a6caa4755  Aether-Portable-0.5.2-win-x64.zip\n");

        Assert.False(ReleaseSource.ChecksumMatches(
            checksums, "Aether-Portable-0.5.2-win-x64.zip", new string('0', 64)));

        // An asset with no published hash is not trusted by default.
        Assert.False(ReleaseSource.ChecksumMatches(checksums, "something-else.zip", new string('f', 64)));
    }

    /// Lines that are not a hash and a name are skipped rather than guessed at.
    [Fact]
    public void Junk_lines_in_the_checksum_file_are_ignored()
    {
        var checksums = ReleaseSource.ParseChecksums(
            "# a comment\n" +
            "\n" +
            "notahash  file.zip\n" +
            "f6955b9b86a6edbcb6aad8de144d8d8d94e6650a0e8d24f5dcaac02a6caa4755 *binary-mode.zip\n");

        Assert.Single(checksums);
        Assert.True(checksums.ContainsKey("binary-mode.zip"));
    }
}
