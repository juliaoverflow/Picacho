using System.Runtime.InteropServices;
using WinRT;

namespace Picacho.Widgets;

/// <summary>
/// Out-of-process COM host launched by the Windows 11 Widgets Board with
/// -RegisterProcessAsComServer.
/// </summary>
internal static class Program
{
    private const uint CLSCTX_LOCAL_SERVER = 0x4;
    private const uint REGCLS_MULTIPLEUSE = 1;

    [STAThread]
    private static void Main(string[] args)
    {
        ComWrappersSupport.InitializeComWrappers();
        var clsid = typeof(WidgetProvider).GUID;
        var factory = new WidgetProviderFactory();
        var hr = CoRegisterClassObject(
            ref clsid,
            factory,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE,
            out var cookie);
        if (hr < 0)
        {
            Marshal.ThrowExceptionForHR(hr);
        }

        try
        {
            // Wait until the Widgets Board terminates the host or SIGINT arrives.
            using var stop = new ManualResetEventSlim(false);
            Console.CancelKeyPress += (_, e) => { e.Cancel = true; stop.Set(); };
            stop.Wait();
        }
        finally
        {
            CoRevokeClassObject(cookie);
        }
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("00000001-0000-0000-C000-000000000046")]
    private interface IClassFactory
    {
        [PreserveSig]
        int CreateInstance(IntPtr pUnkOuter, ref Guid riid, out IntPtr ppvObject);

        [PreserveSig]
        int LockServer([MarshalAs(UnmanagedType.Bool)] bool fLock);
    }

    [ComVisible(true)]
    private sealed class WidgetProviderFactory : IClassFactory
    {
        private static readonly Guid IUnknownGuid = new("00000000-0000-0000-C000-000000000046");

        public int CreateInstance(IntPtr pUnkOuter, ref Guid riid, out IntPtr ppvObject)
        {
            ppvObject = IntPtr.Zero;
            if (pUnkOuter != IntPtr.Zero) return unchecked((int)0x80040110); // CLASS_E_NOAGGREGATION
            var instance = new WidgetProvider();
            if (riid == IUnknownGuid)
            {
                ppvObject = MarshalInspectable<object>.FromManaged(instance);
                return 0;
            }
            ppvObject = MarshalInterface<Microsoft.Windows.Widgets.Providers.IWidgetProvider>.FromManaged(instance);
            return ppvObject == IntPtr.Zero ? unchecked((int)0x80004002) : 0; // E_NOINTERFACE
        }

        public int LockServer(bool fLock) => 0;
    }

    [DllImport("ole32.dll")]
    private static extern int CoRegisterClassObject(
        ref Guid rclsid,
        [MarshalAs(UnmanagedType.IUnknown)] object pUnk,
        uint dwClsContext,
        uint flags,
        out uint lpdwRegister);

    [DllImport("ole32.dll")]
    private static extern int CoRevokeClassObject(uint dwRegister);
}
