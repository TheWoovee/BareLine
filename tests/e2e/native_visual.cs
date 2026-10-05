// SPDX-License-Identifier: MPL-2.0
// Bounded read-only capture of the owned editor client area, never the desktop.
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public sealed class JourneyFrame : IDisposable {
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int left, top, right, bottom; }
    [StructLayout(LayoutKind.Sequential)] struct POINT { public int x, y; }
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr window, ref POINT point);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr window);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr window, IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr dst, int x, int y, int width, int height, IntPtr src, int sx, int sy, uint op);
    public readonly Bitmap Bitmap;
    public readonly int Left, Top;
    JourneyFrame(Bitmap bitmap, int left, int top) { Bitmap = bitmap; Left = left; Top = top; }
    public static JourneyFrame Capture(IntPtr window) {
        RECT rect; POINT origin = new POINT();
        if (!GetClientRect(window, out rect) || !ClientToScreen(window, ref origin)) throw new Exception("Client geometry unavailable");
        int width = rect.right - rect.left, height = rect.bottom - rect.top;
        if (width <= 0 || height <= 0 || width > 4096 || height > 2160 || (long)width * height > 8388608)
            throw new Exception("Client capture outside bounds");
        Bitmap bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
        try {
            using (Graphics graphics = Graphics.FromImage(bitmap)) {
                IntPtr source = GetDC(window), target = graphics.GetHdc();
                try {
                    if (source == IntPtr.Zero || !BitBlt(target, 0, 0, width, height, source, 0, 0, 0x00CC0020))
                        throw new Exception("Owned client capture failed");
                } finally { graphics.ReleaseHdc(target); if (source != IntPtr.Zero) ReleaseDC(window, source); }
            }
            return new JourneyFrame(bitmap, origin.x, origin.y);
        } catch { bitmap.Dispose(); throw; }
    }
    public sealed class PixelCount { public int rgb; public int count; }
    public PixelCount[] Histogram(double[] rectangles) {
        if (rectangles.Length < 4 || rectangles.Length > 128 || rectangles.Length % 4 != 0)
            throw new Exception("Token rectangles outside bounds");
        var pixels = new HashSet<int>();
        var counts = new SortedDictionary<int, int>();
        for (int i = 0; i < rectangles.Length; i += 4) {
            for (int j = 0; j < 4; j++) if (double.IsNaN(rectangles[i+j]) || double.IsInfinity(rectangles[i+j]))
                throw new Exception("Nonfinite token rectangle");
            int x = (int)Math.Floor(rectangles[i] - Left), y = (int)Math.Floor(rectangles[i+1] - Top);
            int right = (int)Math.Ceiling(rectangles[i] + rectangles[i+2] - Left);
            int bottom = (int)Math.Ceiling(rectangles[i+1] + rectangles[i+3] - Top);
            if (x < 0 || y < 0 || right > Bitmap.Width || bottom > Bitmap.Height || right <= x || bottom <= y
                || (long)(right-x)*(bottom-y) > 8192) throw new Exception("Token rectangle escapes owned client or area bound");
            for (int row = y; row < bottom; row++) for (int col = x; col < right; col++) {
                if (!pixels.Add(row*Bitmap.Width+col)) continue;
                if (pixels.Count > 8192) throw new Exception("Token pixel area exceeded 8192");
                int rgb = Bitmap.GetPixel(col, row).ToArgb() & 0xFFFFFF;
                int count; counts.TryGetValue(rgb, out count); counts[rgb] = count + 1;
            }
        }
        var result = new List<PixelCount>();
        foreach (var pair in counts) result.Add(new PixelCount { rgb = pair.Key, count = pair.Value });
        return result.ToArray();
    }
    public void Save(string path) { Bitmap.Save(path, ImageFormat.Png); }
    public void Dispose() { Bitmap.Dispose(); }
}
