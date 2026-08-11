using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text.Json;

namespace Aether.Setup.Services;

/// <summary>Reads what the GitHub releases page is offering.
///
/// Everything here is a pure function over text. Fetching is the caller's job, which keeps the parts that can
/// silently be wrong — is this version newer, is this the right file, does this hash match — testable without a
/// network, and keeps a bad release from being distinguishable from a bad connection.</summary>
public static class ReleaseSource
{
    /// <summary>The asset the launcher downloads: the portable archive, which is aether.exe and its licence.</summary>
    public const string ClientAssetPrefix = "Aether-Portable-";
    public const string ClientAssetSuffix = "-win-x64.zip";

    /// <summary>The file listing each asset's SHA-256, published alongside them.</summary>
    public const string ChecksumAssetName = "SHA256SUMS.txt";

    public sealed record Asset(string Name, string DownloadUrl, long Size);

    public sealed record Release(string Version, IReadOnlyList<Asset> Assets)
    {
        public Asset? ClientArchive => Assets.FirstOrDefault(a =>
            a.Name.StartsWith(ClientAssetPrefix, StringComparison.OrdinalIgnoreCase) &&
            a.Name.EndsWith(ClientAssetSuffix, StringComparison.OrdinalIgnoreCase));

        public Asset? Checksums => Assets.FirstOrDefault(a =>
            a.Name.Equals(ChecksumAssetName, StringComparison.OrdinalIgnoreCase));
    }

    /// <summary>Read a release out of the GitHub API's JSON.
    ///
    /// Returns null rather than throwing on anything unexpected. A launcher that cannot parse the API has to
    /// carry on and start the installed copy; refusing to launch because a web service changed shape would be a
    /// worse failure than not knowing about an update.</summary>
    public static Release? ParseLatest(string json)
    {
        try
        {
            using var document = JsonDocument.Parse(json);
            var root = document.RootElement;

            if (root.ValueKind != JsonValueKind.Object) return null;
            if (!root.TryGetProperty("tag_name", out var tag)) return null;
            if (tag.GetString() is not { Length: > 0 } tagName) return null;

            // Drafts and prereleases are not what a launcher should hand to a player.
            if (root.TryGetProperty("draft", out var draft) && draft.ValueKind == JsonValueKind.True) return null;
            if (root.TryGetProperty("prerelease", out var pre) && pre.ValueKind == JsonValueKind.True) return null;

            var assets = new List<Asset>();
            if (root.TryGetProperty("assets", out var assetArray) && assetArray.ValueKind == JsonValueKind.Array)
            {
                foreach (var element in assetArray.EnumerateArray())
                {
                    var name = element.TryGetProperty("name", out var n) ? n.GetString() : null;
                    var url = element.TryGetProperty("browser_download_url", out var u) ? u.GetString() : null;
                    if (name is null or "" || url is null or "") continue;

                    long size = element.TryGetProperty("size", out var s) && s.TryGetInt64(out var parsed) ? parsed : 0;
                    assets.Add(new Asset(name, url, size));
                }
            }

            return new Release(NormaliseVersion(tagName), assets);
        }
        catch (JsonException)
        {
            return null;
        }
    }

    /// <summary>Strip the leading v from a tag, so <c>v0.5.2</c> and <c>0.5.2</c> are the same version.</summary>
    public static string NormaliseVersion(string version)
    {
        var trimmed = version.Trim();
        return trimmed.StartsWith("v", StringComparison.OrdinalIgnoreCase) ? trimmed[1..] : trimmed;
    }

    /// <summary>Whether <paramref name="candidate"/> is a version worth downloading over <paramref name="installed"/>.
    ///
    /// Compared component by component as numbers, never as text. As strings "0.5.10" sorts below "0.5.9", which
    /// would strand everyone on the older build from the tenth patch onwards and look like the update check
    /// silently doing nothing.</summary>
    public static bool IsNewer(string candidate, string installed)
    {
        var left = Components(candidate);
        var right = Components(installed);
        if (left.Length == 0) return false;
        if (right.Length == 0) return true;

        for (int i = 0; i < Math.Max(left.Length, right.Length); i++)
        {
            long a = i < left.Length ? left[i] : 0;
            long b = i < right.Length ? right[i] : 0;
            if (a != b) return a > b;
        }
        return false;
    }

    private static long[] Components(string version)
    {
        var normalised = NormaliseVersion(version);
        // Anything after a dash is build metadata or a channel, not an ordering we can trust.
        int dash = normalised.IndexOf('-');
        if (dash >= 0) normalised = normalised[..dash];

        var parts = new List<long>();
        foreach (var piece in normalised.Split('.', StringSplitOptions.RemoveEmptyEntries))
        {
            if (!long.TryParse(piece, NumberStyles.None, CultureInfo.InvariantCulture, out var value)) break;
            parts.Add(value);
        }
        return parts.ToArray();
    }

    /// <summary>Read a <c>sha256sum</c> style listing into filename to hash.
    ///
    /// The format is the hash, whitespace, then the name. Lines that are not that are skipped rather than
    /// guessed at: a checksum file we half understood is worse than one we admit we cannot read.</summary>
    public static IReadOnlyDictionary<string, string> ParseChecksums(string text)
    {
        var map = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);

        foreach (var rawLine in text.Split('\n'))
        {
            var line = rawLine.Trim();
            if (line.Length == 0 || line.StartsWith('#')) continue;

            int split = line.IndexOfAny([' ', '\t']);
            if (split <= 0) continue;

            var hash = line[..split];
            if (hash.Length != 64 || !hash.All(Uri.IsHexDigit)) continue;

            // A leading * marks binary mode in some tools and is not part of the name.
            var name = line[split..].TrimStart(' ', '\t', '*').Trim();
            if (name.Length == 0) continue;

            map[name] = hash.ToLowerInvariant();
        }

        return map;
    }

    /// <summary>Whether a downloaded file's hash is the one the release published for it.</summary>
    public static bool ChecksumMatches(IReadOnlyDictionary<string, string> checksums, string assetName, string actualHash)
    {
        if (!checksums.TryGetValue(assetName, out var expected)) return false;
        return string.Equals(expected, actualHash, StringComparison.OrdinalIgnoreCase);
    }
}
