using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

// Guest-only callers are guarded by Scenario.Helpers.ps1. No APIs run on load.
public static class CCUMScenarioDesktop {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct IconId { public uint Size; public IntPtr Window; public uint Id; public Guid Guid; }
    // SPI_GETHIGHCONTRAST returns a borrowed system pointer. A marshalled string
    // would make the CLR free memory it does not own when the call returns.
    [StructLayout(LayoutKind.Sequential)] struct HighContrast { public uint Size, Flags; public IntPtr Scheme; }
    delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumProc callback, IntPtr p);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] static extern bool IsChild(IntPtr parent, IntPtr child);
    [DllImport("user32.dll")] static extern IntPtr GetParent(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll", SetLastError=true)] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, string l, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, StringBuilder l, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll")] static extern uint GetMenuState(IntPtr menu, uint item, uint flags);
    [DllImport("user32.dll")] static extern IntPtr GetSubMenu(IntPtr menu, int item);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetMenuString(IntPtr menu, uint item, StringBuilder s, int size, uint flags);
    [DllImport("user32.dll")] static extern bool GetMenuItemRect(IntPtr window, IntPtr menu, uint item, out Rect rect);
    [DllImport("shell32.dll")] static extern int Shell_NotifyIconGetRect(ref IconId id, out Rect r);
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] static extern bool QueryFullProcessImageName(IntPtr process, uint flags, StringBuilder path, ref uint size);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool SystemParametersInfo(uint action, uint param, ref HighContrast value, uint flags);
    public static bool Exists(long h) { return IsWindow(new IntPtr(h)); }
    public static bool Responding(long h) { IntPtr result; return SendMessageTimeout(new IntPtr(h),0,IntPtr.Zero,IntPtr.Zero,2,1000,out result)!=IntPtr.Zero; }
    public static void UsePhysicalPixels() { if(SetThreadDpiAwarenessContext(new IntPtr(-4))==IntPtr.Zero) throw new Exception("Cannot enable per-monitor DPI awareness for the test thread"); }
    public static Size DisplaySize() { return new Size(GetSystemMetrics(0),GetSystemMetrics(1)); }
    public static void CloseWindow(int processId, string expectedTitle) {
        EnumWindows(delegate(IntPtr h, IntPtr p) {
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner != processId) return true;
            var title = new StringBuilder(256); GetWindowText(h, title, title.Capacity);
            if (title.ToString() != expectedTitle) return true;
            if (!PostMessage(h, 0x10, IntPtr.Zero, IntPtr.Zero)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Cannot close fixture window");
            return false;
        }, IntPtr.Zero);
    }
    public static string ReadUpdateError(int processId) {
        var text=new StringBuilder();
        EnumWindows(delegate(IntPtr h,IntPtr p) {
            uint owner; GetWindowThreadProcessId(h,out owner);
            if(owner!=processId || !IsWindowVisible(h)) return true;
            var title=new StringBuilder(256); GetWindowText(h,title,256);
            if(title.ToString()=="Update failed") {
                EnumChildWindows(h,delegate(IntPtr child,IntPtr unused) {
                    var value=new StringBuilder(4096); GetWindowText(child,value,value.Capacity);
                    text.Append(value).Append(' '); return true;
                },IntPtr.Zero);
            }
            return true;
        },IntPtr.Zero);
        return text.ToString();
    }
    public static bool DismissUpdateError(int processId) {
        bool found=false;
        EnumWindows(delegate(IntPtr h,IntPtr p) {
            uint owner; GetWindowThreadProcessId(h,out owner);
            if(owner!=processId || !IsWindowVisible(h)) return true;
            var title=new StringBuilder(256); GetWindowText(h,title,256);
            if(title.ToString()=="Update failed") {
                IntPtr ok=GetDlgItem(h,1);
                if(!PostMessage(ok!=IntPtr.Zero ? ok : h,ok!=IntPtr.Zero ? 0xf5U : 0x10U,IntPtr.Zero,IntPtr.Zero)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Cannot dismiss helper dialog");
                found=true;
            }
            return true;
        },IntPtr.Zero);
        return found;
    }
    public static string KillDuringReplacement(int pid, string target, long sourceSize, IDisposable gate) {
        var process=Process.GetProcessById(pid);
        var timer=Stopwatch.StartNew();
        gate.Dispose();
        while(timer.Elapsed.TotalSeconds<40 && !process.HasExited) {
            if(System.IO.File.Exists(target+".old")) {
                // CopyFile preallocates the destination to its final length, so
                // length cannot identify a completed copy. The backup remains
                // present until the helper finishes replacement.
                process.Kill(); process.WaitForExit(5000);
                long size=System.IO.File.Exists(target) ? new System.IO.FileInfo(target).Length : 0;
                return "backup present at interruption; target length="+size+"; source length="+sourceSize;
            }
            System.Threading.Thread.Sleep(1);
        }
        throw new Exception("Did not observe an in-progress replacement; interruption coverage not established");
    }
    public static void Post(long h, uint message, long w, long l) {
        if (!PostMessage(new IntPtr(h), message, new IntPtr(w), new IntPtr(l))) throw new Exception("PostMessage failed");
    }
    public static long Dashboard(string executable) {
        long found=0;
        var candidates=new HashSet<uint>();
        foreach(var process in Process.GetProcessesByName(System.IO.Path.GetFileNameWithoutExtension(executable))) {
            using(process) {
                try {
                    uint pid=(uint)process.Id;
                    // .NET Framework MainModule can throw inside its getter if
                    // a short-lived launcher exits during module enumeration.
                    // Query the image without enumerating its loaded modules.
                    IntPtr handle=OpenProcess(0x1000,false,pid);
                    if(handle==IntPtr.Zero) continue;
                    try {
                        var path=new StringBuilder(32768); uint size=(uint)path.Capacity;
                        if(QueryFullProcessImageName(handle,0,path,ref size) &&
                            String.Equals(path.ToString(),executable,StringComparison.OrdinalIgnoreCase)) candidates.Add(pid);
                    } finally { CloseHandle(handle); }
                }
                catch(InvalidOperationException) { }
                catch(System.ComponentModel.Win32Exception) { }
            }
        }
        EnumWindows(delegate(IntPtr h, IntPtr p) {
            uint owner; GetWindowThreadProcessId(h,out owner);
            // Never synchronously query captions on unrelated/hidden UIA windows.
            if(!candidates.Contains(owner) || !IsWindowVisible(h)) return true;
            var s=new StringBuilder(512); GetWindowText(h,s,s.Capacity);
            if (s.ToString()=="Usage Monitor") found=h.ToInt64();
            return true;
        },IntPtr.Zero);
        return found;
    }
    public class HostWindow { public long Handle, Parent; public string Class; public bool Visible; public Rect Bounds; }
    public static HostWindow[] HostWindows(int pid) {
        var result = new List<HostWindow>(); var seen = new HashSet<IntPtr>();
        EnumProc collect = delegate(IntPtr h, IntPtr unused) {
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner != pid || !seen.Add(h)) return true;
            var name = new StringBuilder(256); GetClassName(h, name, name.Capacity);
            if (name.ToString() != "ClaudeCodeUsageMonitor" && name.ToString() != "CCUMDesktopSurface") return true;
            Rect bounds;
            if (GetWindowRect(h, out bounds)) result.Add(new HostWindow { Handle=h.ToInt64(), Parent=GetParent(h).ToInt64(), Class=name.ToString(), Visible=IsWindowVisible(h), Bounds=bounds });
            return true;
        };
        EnumWindows(delegate(IntPtr h, IntPtr unused) { collect(h, unused); EnumChildWindows(h, collect, unused); return true; }, IntPtr.Zero);
        return result.ToArray();
    }
    public static Rect Taskbar() { Rect r; if (!GetWindowRect(FindWindow("Shell_TrayWnd",null),out r)) throw new Exception("No taskbar"); return r; }
    public static IntPtr TaskbarHandle() { return FindWindow("Shell_TrayWnd",null); }
    public static bool IsTaskbarChild(long h) { return IsChild(FindWindow("Shell_TrayWnd",null),new IntPtr(h)); }
    public static int TrayStatus(long h, uint iconId) { Rect r; var id=new IconId { Window=new IntPtr(h), Id=iconId }; id.Size=(uint)Marshal.SizeOf(id); return Shell_NotifyIconGetRect(ref id,out r); }
    public static void HoverTray(long h, uint iconId) {
        Rect r=TrayBounds(h,iconId);
        HoverPoint((r.Left+r.Right)/2,(r.Top+r.Bottom)/2);
    }
    public static void HoverPoint(int x, int y) {
        // Leave keyboard-navigation mode and generate actual pointer input. A
        // bare SetCursorPos can leave the XAML overflow focused without hovering.
        SetCursorPos(10,10);
        mouse_event(1,1,0,0,UIntPtr.Zero);
        System.Threading.Thread.Sleep(100);
        if(!SetCursorPos(x,y)) throw new Exception("Cannot move pointer to tray icon");
        mouse_event(1,1,0,0,UIntPtr.Zero);
    }
    public static void RightClick(long h) {
        Rect r;
        if (!GetWindowRect(new IntPtr(h), out r)) throw new Exception("Right-click target unavailable");
        RightClickPoint((r.Left+r.Right)/2, (r.Top+r.Bottom)/2);
    }
    public static void RightClickPoint(int x, int y) {
        HoverPoint(x,y);
        mouse_event(8,0,0,0,UIntPtr.Zero);
        mouse_event(16,0,0,0,UIntPtr.Zero);
    }
    public static Rect TrayBounds(long h, uint iconId) {
        Rect r; var id=new IconId { Window=new IntPtr(h), Id=iconId }; id.Size=(uint)Marshal.SizeOf(id);
        if (Shell_NotifyIconGetRect(ref id,out r)<0) throw new Exception("Tray icon unavailable");
        return r;
    }
    public static string Tooltip() {
        string text="";
        EnumWindows(delegate(IntPtr h, IntPtr p) {
            var cls=new StringBuilder(256); GetClassName(h,cls,256);
            if (IsWindowVisible(h) && cls.ToString()=="tooltips_class32") {
                var s=new StringBuilder(2048); GetWindowText(h,s,2048);
                if(s.Length==0) { IntPtr result; SendMessageTimeout(h,0xd,new IntPtr(s.Capacity),s,2,500,out result); }
                text += s.ToString();
            }
            return true;
        },IntPtr.Zero);
        return text;
    }
    public static void DismissShellFlyout() { SetCursorPos(10,10); mouse_event(2,0,0,0,UIntPtr.Zero); mouse_event(4,0,0,0,UIntPtr.Zero); }
    public static bool MenuVisible() { return FindWindow("#32768",null)!=IntPtr.Zero; }
    public static bool MenuItemChecked(string label) {
        IntPtr popup=FindWindow("#32768",null), menu;
        if (SendMessageTimeout(popup,0x1e1,IntPtr.Zero,IntPtr.Zero,2,3000,out menu)==IntPtr.Zero) throw new Exception("Menu query timed out");
        var path=new List<uint>();
        if (!FindMenuPath(menu,label,path)) throw new Exception("Menu item not found: "+label);
        for(int depth=0;depth<path.Count-1;depth++) menu=GetSubMenu(menu,(int)path[depth]);
        uint state=GetMenuState(menu,path[path.Count-1],0x400);
        if(state==0xffffffff) throw new Exception("Menu state unavailable");
        return (state&8)!=0;
    }
    public static void ChooseMenuItem(string label) {
        IntPtr popup=FindWindow("#32768",null), menu;
        if (SendMessageTimeout(popup,0x1e1,IntPtr.Zero,IntPtr.Zero,2,3000,out menu)==IntPtr.Zero) throw new Exception("Menu query timed out");
        var path=new List<uint>();
        if (!FindMenuPath(menu,label,path)) throw new Exception("Menu item not found: "+label);
        for(int depth=0;depth<path.Count;depth++) {
            uint i=path[depth];
            Rect r; if (!GetMenuItemRect(IntPtr.Zero,menu,i,out r)) throw new Exception("Menu item bounds unavailable");
            SetCursorPos((r.Left+r.Right)/2,(r.Top+r.Bottom)/2);
            if(depth==path.Count-1) { mouse_event(2,0,0,0,UIntPtr.Zero); mouse_event(4,0,0,0,UIntPtr.Zero); return; }
            // Hover opens native submenus without bypassing the app's action handler.
            System.Threading.Thread.Sleep(1000);
            menu=GetSubMenu(menu,(int)i);
        }
    }
    static bool FindMenuPath(IntPtr menu, string label, List<uint> path) {
        for (uint i=0;i<GetMenuItemCount(menu);i++) {
            var text=new StringBuilder(512); GetMenuString(menu,i,text,512,0x400);
            if (text.ToString().Replace("&","").Equals(label,StringComparison.OrdinalIgnoreCase)) {
                path.Add(i); return true;
            }
            IntPtr sub=GetSubMenu(menu,(int)i);
            if(sub!=IntPtr.Zero) { path.Add(i); if(FindMenuPath(sub,label,path))return true; path.RemoveAt(path.Count-1); }
        }
        return false;
    }
    public static void BroadcastTheme() { IntPtr r; SendMessageTimeout(new IntPtr(0xffff),0x1a,IntPtr.Zero,"ImmersiveColorSet",2,3000,out r); SendMessageTimeout(new IntPtr(0xffff),0x31a,IntPtr.Zero,IntPtr.Zero,2,3000,out r); }
    public static void SetHighContrast(bool enabled) {
        var c=new HighContrast(); c.Size=(uint)Marshal.SizeOf(c);
        if (!SystemParametersInfo(0x42,c.Size,ref c,0)) throw new Exception("SPI_GETHIGHCONTRAST failed");
        if (((c.Flags&1)!=0)==enabled) return;
        c.Flags=enabled ? c.Flags|1U : c.Flags&~1U;
        if (!SystemParametersInfo(0x43,c.Size,ref c,3)) throw new Exception("SPI_SETHIGHCONTRAST failed");
        var actual=new HighContrast(); actual.Size=c.Size;
        if (!SystemParametersInfo(0x42,actual.Size,ref actual,0) || ((actual.Flags&1)!=0)!=enabled) throw new Exception("High contrast did not change");
    }
    public class PixelMetrics { public int DistinctColors, ForegroundPixels; public double Contrast; }
    static double Linear(byte v) { double c=v/255.0; return c<=0.04045 ? c/12.92 : Math.Pow((c+0.055)/1.055,2.4); }
    static double Luminance(Color c) { return .2126*Linear(c.R)+.7152*Linear(c.G)+.0722*Linear(c.B); }
    public static PixelMetrics Capture(long handle, string path) {
        Rect r; if (!GetWindowRect(new IntPtr(handle),out r)) throw new Exception("Window disappeared");
        return CaptureRect(r,path);
    }
    public static PixelMetrics CaptureRegion(long handle, int x, int y, int width, int height, string path) {
        Rect r; if (!GetWindowRect(new IntPtr(handle),out r)) throw new Exception("Window disappeared");
        r.Left+=x; r.Top+=y; r.Right=r.Left+width; r.Bottom=r.Top+height;
        return CaptureRect(r,path);
    }
    static PixelMetrics CaptureRect(Rect r, string path) {
        int width=r.Right-r.Left, height=r.Bottom-r.Top;
        if (width<8 || height<8) throw new Exception("Empty capture bounds");
        using (var b=new Bitmap(width,height)) {
            using (var g=Graphics.FromImage(b)) g.CopyFromScreen(r.Left,r.Top,0,0,new Size(width,height));
            b.Save(path,ImageFormat.Png);
            return MeasureContent(b);
        }
    }
    public static PixelMetrics MeasureContent(Bitmap b) {
            int width=b.Width, height=b.Height;
            // DWM shadows extend several pixels inside GetWindowRect. Exclude
            // the entire frame, not just its outermost two pixels.
            int inset=height>200 ? 16 : 2;
            int top=height>200 ? 48 : 2;
            var colors=new Dictionary<int,int>();
            for(int y=top;y<height-inset;y++) for(int x=inset;x<width-inset;x++) { int c=b.GetPixel(x,y).ToArgb(); if(!colors.ContainsKey(c))colors[c]=0; colors[c]++; }
            int background=0, maximum=0;
            foreach(var pair in colors) if(pair.Value>maximum) {background=pair.Key; maximum=pair.Value;}
            double bg=Luminance(Color.FromArgb(background)), contrast=1; int foreground=0;
            foreach(var pair in colors) {
                double lum=Luminance(Color.FromArgb(pair.Key)); double ratio=(Math.Max(lum,bg)+.05)/(Math.Min(lum,bg)+.05);
                // Reject isolated extrema/noise; record counts as evidence, not a WCAG certification.
                if(pair.Value>=5) contrast=Math.Max(contrast,ratio);
                if(ratio>=1.5) foreground+=pair.Value;
            }
            return new PixelMetrics {DistinctColors=colors.Count,ForegroundPixels=foreground,Contrast=contrast};
    }
    public static int SamplePixel(long handle, int x, int y) {
        Rect r; if (!GetWindowRect(new IntPtr(handle),out r)) throw new Exception("Window disappeared");
        using(var b=new Bitmap(1,1)) { using(var g=Graphics.FromImage(b)) g.CopyFromScreen(r.Left+x,r.Top+y,0,0,new Size(1,1)); return b.GetPixel(0,0).ToArgb() & 0xffffff; }
    }
}
