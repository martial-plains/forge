// SPDX-License-Identifier: MIT OR Apache-2.0
using Microsoft.UI.Xaml;

namespace Forge;

public partial class App : Application
{
    private Window? window;

    public App() => InitializeComponent();

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        // Files named on the command line (or "Open with Forge") are opened.
        var files = Environment.GetCommandLineArgs().Skip(1).Where(a => !a.StartsWith('-')).ToArray();
        var noConfig = Environment.GetCommandLineArgs().Any(a => a is "-q" or "--no-config");
        window = new MainWindow(files, noConfig);
        window.Activate();
    }
}
