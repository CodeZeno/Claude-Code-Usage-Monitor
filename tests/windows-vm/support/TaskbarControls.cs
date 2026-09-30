using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

// Read the native UIA tree, matching the monitor's occupancy reader. The .NET
// Framework proxy loader can omit Win10 task buttons in a PowerShell host.
public static class CCUMTaskbarControls {
    public class Bounds { public double Left, Top, Right, Bottom; }
    public class Control { public string Name; public int Role; public bool Offscreen; public Bounds Rect; }
    // Partial IUnknown interfaces: unused methods preserve native vtable order.
    [ComImport, Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface Automation {
        void CompareElements(); void CompareRuntimeIds(); void GetRootElement();
        void ElementFromHandle(IntPtr hwnd, out Element element);
        void ElementFromPoint(); void GetFocusedElement(); void GetRootElementBuildCache();
        void ElementFromHandleBuildCache(); void ElementFromPointBuildCache(); void GetFocusedElementBuildCache();
        void CreateTreeWalker(); void ControlViewWalker(); void ContentViewWalker(); void RawViewWalker();
        void RawViewCondition(); void ControlViewCondition(); void ContentViewCondition(); void CreateCacheRequest();
        void CreateTrueCondition(out Condition condition);
    }
    [ComImport, Guid("352ffba8-0973-437c-a61f-f64cafd81df9"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface Condition { }
    [ComImport, Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface Element {
        void SetFocus(); void GetRuntimeId(); void FindFirst();
        void FindAll(int scope, Condition condition, out Elements elements);
        void FindFirstBuildCache(); void FindAllBuildCache(); void BuildUpdatedCache();
        void GetCurrentPropertyValue(int property, [MarshalAs(UnmanagedType.Struct)] out object value);
    }
    [ComImport, Guid("14314595-b4bc-4055-95f2-58f2e42c9855"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface Elements {
        void Length(out int length);
        void GetElement(int index, out Element element);
    }
    public static Control[] Read(long hwnd) {
        Control[] result = null; Exception failure = null;
        var thread = new Thread(delegate() {
            Automation automation = null; Element root = null; Condition condition = null; Elements elements = null;
            try {
                automation = (Automation)Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("e22ad333-b25f-460c-83d0-0581107395c9")));
                automation.ElementFromHandle(new IntPtr(hwnd), out root);
                automation.CreateTrueCondition(out condition);
                root.FindAll(4, condition, out elements); // TreeScope_Descendants
                int count; elements.Length(out count);
                var controls = new List<Control>();
                for (int i=0; i<count; i++) {
                    Element element; elements.GetElement(i, out element);
                    try {
                        object name, role, offscreen, rect;
                        element.GetCurrentPropertyValue(30005, out name);
                        element.GetCurrentPropertyValue(30003, out role);
                        element.GetCurrentPropertyValue(30022, out offscreen);
                        element.GetCurrentPropertyValue(30001, out rect);
                        var values = rect as double[];
                        if (values == null || values.Length != 4) continue;
                        controls.Add(new Control { Name=Convert.ToString(name), Role=Convert.ToInt32(role), Offscreen=Convert.ToBoolean(offscreen),
                            Rect=new Bounds { Left=values[0], Top=values[1], Right=values[0]+values[2], Bottom=values[1]+values[3] } });
                    } finally { Marshal.ReleaseComObject(element); }
                }
                result = controls.ToArray();
            } catch (Exception e) { failure = e; }
            finally {
                if (elements != null) Marshal.ReleaseComObject(elements);
                if (condition != null) Marshal.ReleaseComObject(condition);
                if (root != null) Marshal.ReleaseComObject(root);
                if (automation != null) Marshal.ReleaseComObject(automation);
            }
        });
        thread.IsBackground = true; thread.SetApartmentState(ApartmentState.MTA); thread.Start();
        if (!thread.Join(15000)) throw new Exception("Native taskbar UIA read timed out");
        if (failure != null) throw new Exception("Native taskbar UIA read failed", failure);
        return result;
    }
}
