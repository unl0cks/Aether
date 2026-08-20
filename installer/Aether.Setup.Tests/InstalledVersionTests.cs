using System;
using System.IO;
using Aether.Setup.Services;
using Xunit;

namespace Aether.Setup.Tests;

/// <summary>Which version an install records as the one it put on disk.
///
/// A full setup carries its payload, so the two are the same number and nothing can disagree. A launcher
/// carries no payload and installs whatever the releases page is currently offering, so its own version is
/// simply not the answer. Recording it anyway is what made a 0.5.11 launcher install 0.5.12 and then report
/// 0.5.11 as installed, which the next update run reads back and offers to "update" again.</summary>
public sealed class InstalledVersionTests
{
    [Fact]
    public void A_setup_that_carries_its_payload_records_the_version_it_carries()
    {
        Assert.Equal("0.5.12", InstallEngine.InstalledVersion("0.5.12", downloaded: null));
    }

    [Fact]
    public void A_launcher_records_the_version_it_downloaded_rather_than_its_own()
    {
        Assert.Equal("0.5.12", InstallEngine.InstalledVersion("0.5.11", downloaded: "0.5.12"));
    }

    /// <summary>If the download somehow reported nothing, the installer's own version is a wrong answer but a
    /// bounded one. An empty DisplayVersion would leave Add/Remove Programs blank and the update check unable
    /// to compare anything at all.</summary>
    [Fact]
    public void A_download_that_named_no_version_falls_back_rather_than_recording_nothing()
    {
        Assert.Equal("0.5.11", InstallEngine.InstalledVersion("0.5.11", downloaded: ""));
        Assert.Equal("0.5.11", InstallEngine.InstalledVersion("0.5.11", downloaded: "   "));
    }
}

/// <summary>Which version is reported as the one already on disk.
///
/// The registry records what an installer meant to put there, which drifts from what is actually there: Aether
/// updates itself without touching Add/Remove Programs, and a launcher that could not name its download records
/// its own build number instead. Reading the binary is what makes the answer a fact rather than a record, and is
/// what stopped a launcher copied from the 0.6.14 build reporting 0.6.14 to everyone who ran it.</summary>
public sealed class ExistingVersionTests
{
    [Fact]
    public void The_binary_is_believed_over_the_recorded_version()
    {
        Assert.Equal("0.6.31", InstallEngine.PreferBinaryVersion("0.6.31", recorded: "0.6.14"));
    }

    [Fact]
    public void A_binary_that_names_no_version_leaves_the_recorded_one_standing()
    {
        Assert.Equal("0.6.14", InstallEngine.PreferBinaryVersion(null, recorded: "0.6.14"));
        Assert.Equal("0.6.14", InstallEngine.PreferBinaryVersion("", recorded: "0.6.14"));
        Assert.Equal("0.6.14", InstallEngine.PreferBinaryVersion("   ", recorded: "0.6.14"));
    }

    /// <summary>Nothing known from either source stays nothing, rather than becoming an invented number.</summary>
    [Fact]
    public void Neither_source_knowing_reports_nothing()
    {
        Assert.Null(InstallEngine.PreferBinaryVersion(null, recorded: null));
    }

    /// <summary>Build metadata is dropped so a number read off the binary compares equal to the same number
    /// read off the installer, which drops it the same way.</summary>
    [Fact]
    public void Build_metadata_is_trimmed_off_a_version_resource()
    {
        Assert.Equal("0.6.31", InstallEngine.NormalizeVersion("0.6.31+a1b2c3d"));
        Assert.Equal("0.6.31", InstallEngine.NormalizeVersion("  0.6.31  "));
        Assert.Null(InstallEngine.NormalizeVersion(null));
        Assert.Null(InstallEngine.NormalizeVersion("   "));
    }

    /// <summary>Every way of having no binary to read gives null, so the caller falls back rather than
    /// showing a blank or throwing at a player mid-wizard.</summary>
    [Fact]
    public void No_readable_binary_is_null_rather_than_an_error()
    {
        Assert.Null(InstallEngine.BinaryVersion(null));
        Assert.Null(InstallEngine.BinaryVersion(""));
        Assert.Null(InstallEngine.BinaryVersion(Path.Combine(Path.GetTempPath(), "aether-does-not-exist-" + Guid.NewGuid().ToString("N"))));
    }
}
