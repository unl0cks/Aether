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
