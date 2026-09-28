using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class CCUMLabDesktop {
    public class WindowInfo { public long Handle, Parent; public int Left, Top, Right, Bottom; public bool Visible; }
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct APPBARDATA { public uint cbSize; public IntPtr hWnd; public uint callback, edge; public RECT rect; public IntPtr param; }
    delegate bool EnumProc(IntPtr hwnd, IntPtr param);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr param);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr hwnd, EnumProc callback, IntPtr param);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] static extern IntPtr GetParent(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
    public static WindowInfo Tray() {
        IntPtr h = FindWindowEx(FindWindow("Shell_TrayWnd", null), IntPtr.Zero, "TrayNotifyWnd", null);
        RECT r; if (h == IntPtr.Zero || !GetWindowRect(h, out r)) throw new Exception("Tray unavailable");
        return new WindowInfo { Handle=h.ToInt64(), Left=r.Left, Top=r.Top, Right=r.Right, Bottom=r.Bottom };
    }
    public static void Drag(WindowInfo window, int dx) {
        IntPtr h = new IntPtr(window.Handle), result;
        int x = window.Left + 15, y = (window.Top + window.Bottom) / 2;
        SetCursorPos(x, y);
        if (SendMessageTimeout(h, 0x201, new IntPtr(1), IntPtr.Zero, 2, 3000, out result) == IntPtr.Zero) throw new Exception("Drag down timed out");
        SetCursorPos(x + dx, Tray().Bottom - (window.Bottom - window.Top) / 2);
        if (SendMessageTimeout(h, 0x200, new IntPtr(1), IntPtr.Zero, 2, 3000, out result) == IntPtr.Zero) throw new Exception("Drag move timed out");
        if (SendMessageTimeout(h, 0x202, IntPtr.Zero, IntPtr.Zero, 2, 3000, out result) == IntPtr.Zero) throw new Exception("Drag release timed out");
        SetCursorPos(10, 10);
    }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int count);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr h, int index, StringBuilder text, uint length, out uint needed);
    [DllImport("shell32.dll")] static extern UIntPtr SHAppBarMessage(uint message, ref APPBARDATA data);
    public static bool IsDefaultDesktop() {
        IntPtr h = OpenInputDesktop(0, false, 1);
        if (h == IntPtr.Zero) return false;
        try { uint needed; var name = new StringBuilder(256); return GetUserObjectInformation(h, 2, name, 512, out needed) && name.ToString() == "Default"; }
        finally { CloseDesktop(h); }
    }
    public static ulong TaskbarState(bool autoHide) {
        var data = new APPBARDATA(); data.cbSize = (uint)Marshal.SizeOf(data); data.hWnd = FindWindow("Shell_TrayWnd", null);
        if (data.hWnd == IntPtr.Zero) throw new Exception("Explorer taskbar unavailable");
        ulong state = SHAppBarMessage(4, ref data).ToUInt64();
        data.param = new IntPtr((long)(autoHide ? state | 1UL : state & ~1UL));
        SHAppBarMessage(10, ref data);
        return SHAppBarMessage(4, ref data).ToUInt64();
    }
    public static WindowInfo[] WindowsForProcess(int pid) {
        var list = new List<WindowInfo>(); var seen = new HashSet<IntPtr>();
        EnumProc collect = delegate(IntPtr h, IntPtr p) {
            uint owner; GetWindowThreadProcessId(h, out owner);
            var name = new StringBuilder(256); GetClassName(h, name, name.Capacity);
            RECT r;
            if (owner == pid && name.ToString() == "ClaudeCodeUsageMonitor" && seen.Add(h) && GetWindowRect(h, out r))
                list.Add(new WindowInfo { Handle=h.ToInt64(), Parent=GetParent(h).ToInt64(), Left=r.Left, Top=r.Top, Right=r.Right, Bottom=r.Bottom, Visible=IsWindowVisible(h) });
            return true;
        };
        EnumWindows(delegate(IntPtr h, IntPtr p) { collect(h,p); EnumChildWindows(h, collect, p); return true; }, IntPtr.Zero);
        return list.ToArray();
    }
}
