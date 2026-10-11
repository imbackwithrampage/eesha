// The Win32 calls the helper makes itself: the pipe with exactly the flags and
// the access list docs/ARCHITECTURE.md §11 names, who is on its other end, and
// word of the machine waking up.

using System.ComponentModel;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;

namespace NotchHw;

internal static class Native
{
    private const uint PIPE_ACCESS_DUPLEX = 0x3;
    private const uint FILE_FLAG_FIRST_PIPE_INSTANCE = 0x0008_0000;
    private const uint FILE_FLAG_OVERLAPPED = 0x4000_0000;
    private const uint PIPE_TYPE_MESSAGE = 0x4;
    private const uint PIPE_READMODE_MESSAGE = 0x2;
    private const uint PIPE_REJECT_REMOTE_CLIENTS = 0x8;
    private const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    private const uint SYNCHRONIZE = 0x0010_0000;
    private const uint TOKEN_QUERY = 0x8;
    private const uint WAIT_TIMEOUT = 0x102;

    [StructLayout(LayoutKind.Sequential)]
    private struct SECURITY_ATTRIBUTES
    {
        public int nLength;
        public IntPtr lpSecurityDescriptor;
        public int bInheritHandle;
    }

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern SafePipeHandle CreateNamedPipeW(string name, uint openMode, uint pipeMode, uint maxInstances, uint outBuffer, uint inBuffer, uint defaultTimeout, ref SECURITY_ATTRIBUTES attributes);

    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string sddl, uint revision, out IntPtr descriptor, IntPtr size);

    [DllImport("kernel32.dll")]
    private static extern IntPtr LocalFree(IntPtr memory);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetNamedPipeClientProcessId(SafePipeHandle pipe, out uint pid);

    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "OpenProcess")]
    private static extern IntPtr OpenProcessRaw(uint access, bool inherit, uint pid);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern bool QueryFullProcessImageNameW(IntPtr process, uint flags, [Out] char[] name, ref uint size);

    [DllImport("kernel32.dll")]
    private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);

    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr handle);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);

    /// The PawnIO driver answers: its device opens, the way the sensor library
    /// opens it (read and write, shared, an existing device). Nothing is sent to it.
    public static bool DriverAnswers()
    {
        const uint READ_WRITE = 0x3, SHARE_READ_WRITE = 0x3, OPEN_EXISTING = 3, FILE_ATTRIBUTE_NORMAL = 0x80;
        using var device = CreateFileW(@"\\?\GLOBALROOT\Device\PawnIO", READ_WRITE, SHARE_READ_WRITE, IntPtr.Zero, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, IntPtr.Zero);
        return !device.IsInvalid;
    }

    private delegate uint ResumeCallback(IntPtr context, uint type, IntPtr setting);

    [StructLayout(LayoutKind.Sequential)]
    private struct DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS
    {
        public ResumeCallback Callback;
        public IntPtr Context;
    }

    [DllImport("powrprof.dll")]
    private static extern uint PowerRegisterSuspendResumeNotification(uint flags, ref DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS recipient, out IntPtr registration);

    /// The one instance of the pipe there will ever be: first-instance (a name
    /// somebody already serves is an error, not a second server), message
    /// mode, local clients only, and an access list of exactly three: SYSTEM,
    /// the administrators, and `sid`, the user who launched the helper. That
    /// user may read and write and nothing more: not create an instance, not
    /// change the list (the mask is read + write without FILE_CREATE_PIPE_INSTANCE,
    /// and the owner is held to reading the list).
    public static NamedPipeServerStream CreatePipe(string name, string sid)
    {
        var sddl = $"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;{sid})(A;;RC;;;OW)";
        if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, out var descriptor, IntPtr.Zero))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "the pipe's access list");
        try
        {
            var attributes = new SECURITY_ATTRIBUTES { nLength = Marshal.SizeOf<SECURITY_ATTRIBUTES>(), lpSecurityDescriptor = descriptor };
            var handle = CreateNamedPipeW(
                name,
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                256 * 1024,
                8 * 1024,
                0,
                ref attributes);
            if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), "the pipe");
            return new NamedPipeServerStream(PipeDirection.InOut, true, false, handle);
        }
        finally
        {
            LocalFree(descriptor);
        }
    }

    public static uint? ClientPid(NamedPipeServerStream pipe) => GetNamedPipeClientProcessId(pipe.SafePipeHandle, out var pid) ? pid : null;

    /// A handle that can be asked for the image path and waited on, or zero.
    /// While it is open the process's PID belongs to nobody else.
    public static IntPtr OpenProcess(uint pid) => OpenProcessRaw(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, false, pid);

    public static bool Exited(IntPtr process) => WaitForSingleObject(process, 0) != WAIT_TIMEOUT;

    /// The full path of the file the process runs, or null.
    public static string? ImagePath(IntPtr process)
    {
        var buffer = new char[32768];
        var size = (uint)buffer.Length;
        return QueryFullProcessImageNameW(process, 0, buffer, ref size) ? new string(buffer, 0, (int)size) : null;
    }

    /// The SID of the account the process runs as.
    public static string? UserSid(IntPtr process)
    {
        if (!OpenProcessToken(process, TOKEN_QUERY, out var token)) return null;
        try
        {
            using var identity = new WindowsIdentity(token);
            return identity.User?.Value;
        }
        finally
        {
            CloseHandle(token);
        }
    }

    // The delegate has to outlive the registration: the system calls it for as long as the process runs.
    private static ResumeCallback? resume;

    /// `woke` is called, on a system thread, each time the machine resumes.
    public static void OnResume(Action woke)
    {
        const uint DEVICE_NOTIFY_CALLBACK = 2;
        const uint PBT_APMRESUMEAUTOMATIC = 0x12;
        resume = (_, type, _) =>
        {
            if (type == PBT_APMRESUMEAUTOMATIC) woke();
            return 0;
        };
        var recipient = new DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback = resume };
        PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, ref recipient, out _);
    }
}
