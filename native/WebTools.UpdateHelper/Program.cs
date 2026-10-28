using System.ComponentModel;
using System.Reflection;
using System.Runtime.InteropServices;

namespace WebTools.UpdateHelper;

internal static class Program
{
    private static async Task<int> Main(string[] args)
    {
        if (args.Length == 1 && args[0] == "--find-rust-preview")
        {
            var root = FindRustPreviewFromShortcut();
            if (root is null) return 3;
            Console.Write(root);
            return 0;
        }
        if (args.Length != 2 || args[0] is not ("--check-install" or "--prepare-install"))
        {
            Console.Error.WriteLine("用法：WebTools.UpdateHelper.exe --check-install|--prepare-install <安装目录>");
            return 2;
        }

        try
        {
            var root = InstallProcessTargets.NormalizeInstallRoot(args[1]);
            if (args[0] == "--prepare-install")
            {
                var prepared = await new InstallPreparationService().PrepareAsync(root, TimeSpan.FromSeconds(35)).ConfigureAwait(false);
                if (!prepared) Console.Error.WriteLine("WebTools 未能正常退出。请从托盘退出后，在当前安装窗口继续重试。");
                return prepared ? 0 : 12;
            }
            var snapshot = InstallProcessTargets.Discover(root);
            if (snapshot.UnresolvedCandidateProcessIds.Count > 0)
            {
                Console.Error.WriteLine("无法确认 WebTools 进程是否已退出，请正常退出后台后重试。");
                return 11;
            }
            if (snapshot.Targets.Count > 0)
            {
                Console.Error.WriteLine("WebTools 仍在运行，请从系统托盘正常退出后重试。");
                return 10;
            }
            return 0;
        }
        catch (OperationCanceledException)
        {
            Console.Error.WriteLine("等待 WebTools 退出超时；未卸载或替换文件，可以在当前安装窗口重试。");
            return 12;
        }
        catch (Exception error) when (error is ArgumentException or NotSupportedException or IOException or UnauthorizedAccessException or InvalidOperationException or Win32Exception)
        {
            Console.Error.WriteLine("无法安全检查 WebTools 安装目录，请退出后台后重试。");
            return 11;
        }
    }

    private static string? FindRustPreviewFromShortcut()
    {
        var shortcutPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory), "WebTools Rust Preview.lnk");
        if (!File.Exists(shortcutPath)) return null;
        object? shell = null;
        object? shortcut = null;
        try
        {
            var shellType = Type.GetTypeFromProgID("WScript.Shell");
            if (shellType is null || (shell = Activator.CreateInstance(shellType)) is null) return null;
            shortcut = shell.GetType().InvokeMember("CreateShortcut", BindingFlags.InvokeMethod, null, shell, [shortcutPath]);
            var target = shortcut?.GetType().InvokeMember("TargetPath", BindingFlags.GetProperty, null, shortcut, null)?.ToString();
            if (string.IsNullOrWhiteSpace(target) || !Path.IsPathFullyQualified(target)) return null;
            var executable = Path.GetFullPath(target);
            var root = Path.GetDirectoryName(executable);
            if (root is null || !Path.GetFileName(executable).Equals("WebTools.NativeHost.exe", StringComparison.OrdinalIgnoreCase)) return null;
            return File.Exists(executable) && File.Exists(Path.Combine(root, ".phase4f-test-install")) && File.Exists(Path.Combine(root, "Uninstall.exe")) ? root : null;
        }
        catch (Exception error) when (error is COMException or TargetInvocationException or IOException or UnauthorizedAccessException or ArgumentException)
        {
            return null;
        }
        finally
        {
            if (shortcut is not null && Marshal.IsComObject(shortcut)) Marshal.FinalReleaseComObject(shortcut);
            if (shell is not null && Marshal.IsComObject(shell)) Marshal.FinalReleaseComObject(shell);
        }
    }
}
