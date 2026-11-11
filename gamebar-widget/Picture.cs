// Aimdot Game Bar widget - reading what the Aimdot app writes into this package's LocalState folder
// (overlay/src/gamebar.rs is the writer; keep the two in step).
//
//   crosshair.ini        format, seq, visible, width/height, offset, recenter, rainbow settings
//   crosshair.png        the crosshair, 1 image pixel = 1 screen pixel
//   crosshair-base.png   rainbow only: the part that keeps its colour (outline), RGBA
//   crosshair-fill.png   rainbow only: coverage (0..255) of the part that gets the rainbow colour, grayscale
//
// Every finished picture = base + fill * colour (premultiplied), which is exactly how the app's engine colours a
// crosshair, so the rainbow here looks the same as in the normal overlay.
using System;
using System.Globalization;
using System.Threading.Tasks;
using Windows.Graphics.Imaging;
using Windows.Storage;
using Windows.Storage.Streams;

namespace CrosshairWidget
{
    /// <summary>crosshair.ini</summary>
    sealed class WidgetState
    {
        public int Format;
        public long Seq = -1;
        public bool Visible;
        public int OffsetX, OffsetY;
        public long Recenter;
        public bool Rainbow;
        public int RainbowMode;          // 0 cycle, 1 wave, 2 spin (same as the app)
        public double RainbowSpeed = 1;
        public double RainbowSat = 1;

        public static WidgetState Parse(string text)
        {
            var s = new WidgetState();
            foreach (var raw in text.Split('\n'))
            {
                var line = raw.Trim();
                if (line.Length == 0 || line[0] == '#') continue;
                int eq = line.IndexOf('=');
                if (eq <= 0) continue;
                string k = line.Substring(0, eq).Trim(), v = line.Substring(eq + 1).Trim();
                switch (k)
                {
                    case "format": s.Format = Int(v); break;
                    case "seq": s.Seq = Long(v); break;
                    case "visible": s.Visible = v == "1"; break;
                    case "offset_x": s.OffsetX = Int(v); break;
                    case "offset_y": s.OffsetY = Int(v); break;
                    case "recenter": s.Recenter = Long(v); break;
                    case "rainbow": s.Rainbow = v == "1"; break;
                    case "rainbow_mode": s.RainbowMode = Int(v); break;
                    case "rainbow_speed": s.RainbowSpeed = Num(v, 1); break;
                    case "rainbow_sat": s.RainbowSat = Num(v, 1); break;
                }
            }
            return s;
        }

        static int Int(string v) { int x; return int.TryParse(v, NumberStyles.Integer, CultureInfo.InvariantCulture, out x) ? x : 0; }
        static long Long(string v) { long x; return long.TryParse(v, NumberStyles.Integer, CultureInfo.InvariantCulture, out x) ? x : 0; }
        static double Num(string v, double d) { double x; return double.TryParse(v, NumberStyles.Float, CultureInfo.InvariantCulture, out x) && !double.IsNaN(x) ? x : d; }
    }

    /// <summary>Decoded picture: BGRA8, 4 bytes per pixel.</summary>
    sealed class Pixels
    {
        public int W, H;
        public byte[] Bgra;
    }

    static class Picture
    {
        /// <summary>
        /// Reads a PNG from the folder. The whole file is read into memory first so the file is open only for a
        /// moment (the app renames new files over it). Null if it's missing or can't be read right now.
        /// </summary>
        public static async Task<Pixels> LoadAsync(StorageFolder folder, string name, BitmapAlphaMode alpha)
        {
            try
            {
                var file = await folder.TryGetItemAsync(name) as StorageFile;
                if (file == null) return null;
                IBuffer buf = await FileIO.ReadBufferAsync(file);
                using (var mem = new InMemoryRandomAccessStream())
                {
                    await mem.WriteAsync(buf);
                    mem.Seek(0);
                    var dec = await BitmapDecoder.CreateAsync(BitmapDecoder.PngDecoderId, mem);
                    var data = await dec.GetPixelDataAsync(
                        BitmapPixelFormat.Bgra8, alpha, new BitmapTransform(),
                        ExifOrientationMode.IgnoreExifOrientation, ColorManagementMode.DoNotColorManage);
                    return new Pixels { W = (int)dec.PixelWidth, H = (int)dec.PixelHeight, Bgra = data.DetachPixelData() };
                }
            }
            catch (Exception)
            {
                return null;
            }
        }
    }

    /// <summary>The rainbow animation, done here so the app doesn't have to write files 30 times a second.
    /// Same maths as the app's engine (core/src/crosshair.rs, Tint).</summary>
    sealed class Rainbow
    {
        readonly byte[] basePx;   // premultiplied BGRA
        readonly byte[] fill;     // 0..255 per pixel
        readonly float[] hueAt;   // wave / spin: hue offset per pixel
        readonly int mode;
        readonly float sat, speed;
        public readonly int W, H;

        public Rainbow(Pixels b, Pixels f, WidgetState s)
        {
            W = b.W; H = b.H;
            basePx = b.Bgra;
            int n = W * H;
            fill = new byte[n];
            for (int i = 0; i < n; i++) fill[i] = f.Bgra[i * 4];  // gray decodes to B = G = R
            mode = Math.Max(0, Math.Min(2, s.RainbowMode));
            sat = (float)Math.Max(0.2, Math.Min(1.0, s.RainbowSat));
            speed = (float)Math.Max(0.1, Math.Min(5.0, s.RainbowSpeed));
            if (mode != 0)
            {
                hueAt = new float[n];
                float hx = W / 2f, hy = H / 2f;
                for (int y = 0; y < H; y++)
                    for (int x = 0; x < W; x++)
                    {
                        float dx = x + 0.5f - hx, dy = y + 0.5f - hy;
                        hueAt[y * W + x] = mode == 1
                            ? -(dx + dy) * 0.012f
                            : (float)(Math.Atan2(dy, dx) / (2 * Math.PI));
                    }
            }
        }

        public static bool Fits(Pixels b, Pixels f)
        {
            return b != null && f != null && b.W == f.W && b.H == f.H && b.W > 0 && b.H > 0;
        }

        /// <summary>Colours one frame at time t (seconds) into dst (premultiplied BGRA, W*H*4 bytes).</summary>
        public void Render(double t, byte[] dst)
        {
            double h0 = t * speed * 0.2;
            float hue = (float)(h0 - Math.Floor(h0));
            int r = 0, g = 0, b = 0;
            if (mode == 0) Hsv(hue, sat, out r, out g, out b);
            int n = W * H;
            for (int i = 0; i < n; i++)
            {
                int j = i * 4;
                int f = fill[i];
                if (f == 0)
                {
                    dst[j] = basePx[j]; dst[j + 1] = basePx[j + 1]; dst[j + 2] = basePx[j + 2]; dst[j + 3] = basePx[j + 3];
                    continue;
                }
                if (mode != 0) Hsv(hue + hueAt[i], sat, out r, out g, out b);
                dst[j] = Clamp(basePx[j] + (f * b + 127) / 255);
                dst[j + 1] = Clamp(basePx[j + 1] + (f * g + 127) / 255);
                dst[j + 2] = Clamp(basePx[j + 2] + (f * r + 127) / 255);
                dst[j + 3] = Clamp(basePx[j + 3] + f);
            }
        }

        static byte Clamp(int v) { return (byte)(v > 255 ? 255 : v); }

        // Same as Color::from_hsv in core/src/gfx.rs (v = 1).
        static void Hsv(float h, float s, out int r, out int g, out int b)
        {
            h = (h - (float)Math.Floor(h)) * 6f;
            float i = (float)Math.Floor(h);
            float f = h - i;
            float p = 1f - s, q = 1f - s * f, t = 1f - s * (1f - f);
            float rr, gg, bb;
            switch ((int)i)
            {
                case 0: rr = 1; gg = t; bb = p; break;
                case 1: rr = q; gg = 1; bb = p; break;
                case 2: rr = p; gg = 1; bb = t; break;
                case 3: rr = p; gg = q; bb = 1; break;
                case 4: rr = t; gg = p; bb = 1; break;
                default: rr = 1; gg = p; bb = q; break;
            }
            r = (int)(Math.Max(0f, Math.Min(1f, rr)) * 255f + 0.5f);
            g = (int)(Math.Max(0f, Math.Min(1f, gg)) * 255f + 0.5f);
            b = (int)(Math.Max(0f, Math.Min(1f, bb)) * 255f + 0.5f);
        }
    }
}
