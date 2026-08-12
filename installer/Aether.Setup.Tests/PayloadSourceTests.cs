using System;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Aether.Setup;
using Aether.Setup.Services;
using Xunit;

namespace Aether.Setup.Tests;

/// <summary>Where an install's files come from, and what happens when the archive lies.
///
/// A full setup carries its payload compressed inside itself. A launcher carries nothing and downloads the
/// published portable zip instead. The third build, the bare one, carries nothing and cannot install anything
/// because it is the uninstaller. Two of those three have no payload, so "no payload" alone can no longer mean
/// "uninstall", which is what these pin down.</summary>
public sealed class PayloadSourceTests
{
    private static string TempDir()
    {
        string path = Path.Combine(Path.GetTempPath(), "aether-payload-test-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(path);
        return path;
    }

    private static string ZipContaining(string dir, params (string Name, string Body)[] entries)
    {
        string path = Path.Combine(dir, "archive.zip");
        using var file = new FileStream(path, FileMode.Create);
        using var zip = new ZipArchive(file, ZipArchiveMode.Create);
        foreach (var (name, body) in entries)
        {
            var entry = zip.CreateEntry(name);
            using var stream = entry.Open();
            stream.Write(Encoding.UTF8.GetBytes(body));
        }
        return path;
    }

    [Fact]
    public async Task A_downloaded_archive_unpacks_into_the_install_folder()
    {
        string work = TempDir();
        string target = Path.Combine(work, "install");
        string archive = ZipContaining(work, ("aether.exe", "binary"), ("LICENSE.md", "text"));

        try
        {
            await Payload.ExtractZipAsync(archive, target, null, null, CancellationToken.None);

            Assert.Equal("binary", await File.ReadAllTextAsync(Path.Combine(target, "aether.exe")));
            Assert.Equal("text", await File.ReadAllTextAsync(Path.Combine(target, "LICENSE.md")));
        }
        finally { Directory.Delete(work, recursive: true); }
    }

    [Fact]
    public async Task Nested_folders_in_the_archive_are_recreated()
    {
        string work = TempDir();
        string target = Path.Combine(work, "install");
        string archive = ZipContaining(work, ("video/openh264.dll", "codec"));

        try
        {
            await Payload.ExtractZipAsync(archive, target, null, null, CancellationToken.None);
            Assert.True(File.Exists(Path.Combine(target, "video", "openh264.dll")));
        }
        finally { Directory.Delete(work, recursive: true); }
    }

    /// <summary>The archive is ours and its hash was verified, but an installer that writes wherever a path
    /// inside an archive tells it to is a hole regardless of who built the file. The embedded payload already
    /// refuses this; the downloaded one has to as well, and it is the one that arrives over the network.</summary>
    [Fact]
    public async Task An_entry_that_escapes_the_install_folder_is_refused()
    {
        string work = TempDir();
        string target = Path.Combine(work, "install");
        string archive = ZipContaining(work, ("../../evil.exe", "payload"));

        try
        {
            await Assert.ThrowsAsync<InvalidOperationException>(() =>
                Payload.ExtractZipAsync(archive, target, null, null, CancellationToken.None));

            Assert.False(File.Exists(Path.Combine(work, "evil.exe")));
            Assert.False(File.Exists(Path.Combine(Path.GetDirectoryName(work)!, "evil.exe")));
        }
        finally { Directory.Delete(work, recursive: true); }
    }

    [Fact]
    public async Task Progress_reaches_one_when_the_archive_is_unpacked()
    {
        string work = TempDir();
        string target = Path.Combine(work, "install");
        string archive = ZipContaining(work, ("a", "one"), ("b", "two"), ("c", "three"));
        var seen = new System.Collections.Generic.List<double>();

        try
        {
            await Payload.ExtractZipAsync(archive, target, new Progress<double>(seen.Add), null, CancellationToken.None);
            Assert.Contains(1.0, seen);
            Assert.All(seen, value => Assert.InRange(value, 0.0, 1.0));
        }
        finally { Directory.Delete(work, recursive: true); }
    }

    /// <summary>The three builds, told apart. A test assembly builds against the default, which is the bare
    /// one, so this also proves the flag has to be opted into rather than defaulting on.</summary>
    [Fact]
    public void A_build_is_not_a_launcher_unless_it_was_built_as_one()
    {
        Assert.False(SetupInfo.IsLauncher);
    }

    [Fact]
    public void A_build_with_no_payload_and_no_launcher_flag_is_the_uninstaller()
    {
        Assert.False(Payload.Exists);
        Assert.Equal(InstallEngine.FileSource.None, InstallEngine.SourceOfFiles);
    }

    /// <summary>The launcher needs somewhere to put what it downloads, and it must not be the install folder:
    /// a hundred megabyte zip left in Program Files after every install is litter, and writing it there before
    /// the install is confirmed would be worse.</summary>
    [Fact]
    public void The_download_lands_outside_the_install_folder()
    {
        string staged = InstallEngine.StagedArchivePath("0.5.11");

        Assert.EndsWith(".zip", staged, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("0.5.11", staged);
        Assert.StartsWith(Path.GetTempPath(), staged, StringComparison.OrdinalIgnoreCase);
    }
}
