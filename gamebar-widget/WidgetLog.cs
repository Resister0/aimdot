// Aimdot Game Bar widget - its own small log, LocalState\widget.log (next to crosshair.ini). The same line shape as
// the app's aimdot.log (core/src/log.rs):  "2026-11-10 14:03:10Z [area] message"  (UTC), so the app can copy the
// widget's lines into aimdot.log as "[widget/area] ..." while Game Bar mode is on. That way one log shows both
// sides of Game Bar mode - the only way to see what happened over an exclusive-fullscreen game.
//
// Events only (load, new ini, a picture that can't be read, pinned / click-through / display mode changes), never
// per frame. Over 64 KB it moves to widget.log.1. The widget watches only *.ini files, so writing here never
// triggers a reload. Errors are swallowed: the log must never break the widget.
using System;
using System.Globalization;
using System.IO;
using System.Text;
using Windows.Storage;

namespace CrosshairWidget
{
    static class WidgetLog
    {
        public const string FileName = "widget.log";
        const long MaxBytes = 64 * 1024;
        static readonly object gate = new object();
        static string path;
        static string last;     // last message, to drop exact repeats (e.g. the same read error on every retry)
        static int again;

        static string Path
        {
            get
            {
                if (path == null)
                {
                    try { path = System.IO.Path.Combine(ApplicationData.Current.LocalFolder.Path, FileName); }
                    catch (Exception) { path = ""; }
                }
                return path;
            }
        }

        /// <summary>One line: "[area] message". Exact repeats of the previous line are counted, not written.</summary>
        public static void Line(string area, string message)
        {
            lock (gate)
            {
                try
                {
                    string msg = "[" + area + "] " + (message ?? "").Replace('\r', ' ').Replace('\n', ' ');
                    if (msg == last) { again++; return; }
                    if (again > 0) Write("[" + area + "] (the line before came " + again + " more times)");
                    last = msg;
                    again = 0;
                    Write(msg);
                }
                catch (Exception)
                {
                }
            }
        }

        static void Write(string msg)
        {
            string p = Path;
            if (string.IsNullOrEmpty(p)) return;
            var fi = new FileInfo(p);
            if (fi.Exists && fi.Length >= MaxBytes)
            {
                string old = p + ".1";
                if (File.Exists(old)) File.Delete(old);
                File.Move(p, old);
            }
            string stamp = DateTime.UtcNow.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture) + "Z";
            File.AppendAllText(p, stamp + " " + msg + "\r\n", new UTF8Encoding(false));
        }

        /// <summary>An exception as one short phrase (type + message, no stack, no paths beyond the package).</summary>
        public static string Error(Exception e)
        {
            if (e == null) return "";
            string m = e.Message ?? "";
            int nl = m.IndexOfAny(new[] { '\r', '\n' });
            if (nl >= 0) m = m.Substring(0, nl);
            return e.GetType().Name + ": " + m.Trim() + " (0x" + e.HResult.ToString("X8", CultureInfo.InvariantCulture) + ")";
        }
    }
}
