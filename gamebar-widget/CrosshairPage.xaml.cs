// Aimdot Game Bar widget - the one page. Draws crosshair.png from this package's LocalState folder at the
// centre of the SCREEN (not of the widget window), reloads when the Aimdot app writes new files (file query
// ContentsChanged - no polling), re-tints itself for the rainbow, and shows / hides on the ini's "visible".
// What it does goes to LocalState\widget.log (WidgetLog.cs): load, each new ini, pictures it can't read, pinned /
// click-through / Game Bar open-closed changes - events only, never per frame.
using System;
using System.Diagnostics;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Threading.Tasks;
using Microsoft.Gaming.XboxGameBar;
using Windows.ApplicationModel;
using Windows.Foundation;
using Windows.Graphics.Display;
using Windows.Graphics.Imaging;
using Windows.Storage;
using Windows.Storage.Search;
using Windows.UI.Core;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Media.Imaging;
using Windows.UI.Xaml.Navigation;

namespace CrosshairWidget
{
    public sealed partial class CrosshairPage : Page
    {
        XboxGameBarWidget widget;                          // null when not started by Game Bar (F5 in VS)
        readonly StorageFolder folder = ApplicationData.Current.LocalFolder;
        StorageFileQueryResult query;                      // kept alive, or ContentsChanged stops
        WidgetState state = new WidgetState();
        long seq = -1;
        long recenter = -1;
        int pixW, pixH;                                    // picture size in screen pixels
        WriteableBitmap bitmap;
        byte[] frame;
        Rainbow rainbow;
        readonly DispatcherTimer anim = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(33) };
        readonly Stopwatch clock = Stopwatch.StartNew();
        bool loading, loadAgain;
        int misses;                                        // crosshair.png missing right after a new ini
        bool logPlace;                                     // log where the next Place() puts the picture
        string lastHint;                                   // the hint last logged

        public CrosshairPage()
        {
            InitializeComponent();
            anim.Tick += (s, e) => DrawRainbow();
        }

        protected override async void OnNavigatedTo(NavigationEventArgs e)
        {
            widget = e.Parameter as XboxGameBarWidget;
            LogLoad();
            if (widget != null)
            {
                // Game Bar raises these on other threads: always hop to the UI thread.
                widget.WindowBoundsChanged += (s, a) => OnUi(Place);
                widget.VisibleChanged += (s, a) => OnUi(() =>
                {
                    WidgetLog.Line("gamebar", widget.Visible ? "widget visible" : "widget hidden");
                    Chrome(); Animate(); Reload();
                });
                widget.GameBarDisplayModeChanged += (s, a) => OnUi(() =>
                {
                    WidgetLog.Line("gamebar", DisplayMode() + ", widget " + (widget.Pinned ? "pinned" : "NOT pinned") +
                        ", click-through " + (widget.ClickThroughEnabled ? "on" : "OFF"));
                    logPlace = true;
                    Chrome(); Place();
                });
                widget.PinnedChanged += (s, a) => OnUi(() =>
                {
                    WidgetLog.Line("gamebar", widget.Pinned ? "pinned" : "unpinned (the crosshair goes when Game Bar closes)");
                    logPlace = true;
                    Chrome(); Place();
                });
                widget.ClickThroughEnabledChanged += (s, a) => OnUi(() =>
                {
                    WidgetLog.Line("gamebar", widget.ClickThroughEnabled ? "click-through on" : "click-through off (the widget takes the mouse while pinned)");
                    Chrome();
                });
            }
            SizeChanged += (s, a) => Place();
            DisplayInformation.GetForCurrentView().DpiChanged += (s, a) => { logPlace = true; Place(); };
            Application.Current.Resuming += (s, a) => OnUi(() =>
            {
                WidgetLog.Line("load", "resumed");
                Reload(); // changes made while suspended aren't reported
            });

            await Watch();
            Reload();
            Chrome();

            // Opened from the Game Bar menu: put the window in the middle of the screen, where the crosshair goes.
            if (widget != null && widget.GameBarDisplayMode == XboxGameBarDisplayMode.Foreground)
            {
                await CenterAsync();
            }
        }

        void OnUi(Action a)
        {
            var _ = Dispatcher.RunAsync(CoreDispatcherPriority.Normal, () => a());
        }

        // ---- Log -------------------------------------------------------------------------------------------------

        void LogLoad()
        {
            string ver = "?";
            try { var v = Package.Current.Id.Version; ver = v.Major + "." + v.Minor + "." + v.Build + "." + v.Revision; }
            catch (Exception) { }
            var di = DisplayInformation.GetForCurrentView();
            string where = widget == null
                ? "outside Game Bar (Visual Studio F5) - only a hint is shown"
                : DisplayMode() + ", " + (widget.Pinned ? "pinned" : "NOT pinned") + ", click-through " +
                  (widget.ClickThroughEnabled ? "on" : "OFF") + ", " + (widget.Visible ? "visible" : "hidden");
            WidgetLog.Line("load", "Aimdot widget " + ver + " loaded - " + where + "; screen " + di.ScreenWidthInRawPixels +
                "x" + di.ScreenHeightInRawPixels + " px, scale " + di.RawPixelsPerViewPixel.ToString("0.##", System.Globalization.CultureInfo.InvariantCulture));
        }

        string DisplayMode()
        {
            if (widget == null) return "no Game Bar";
            return widget.GameBarDisplayMode == XboxGameBarDisplayMode.Foreground ? "Game Bar open" : "Game Bar closed (game in front)";
        }

        // ---- Watching the folder -------------------------------------------------------------------------------

        async Task Watch()
        {
            try
            {
                // Only .ini files: the app writes the pictures first and the ini last, so one event per change, and
                // the ".tmp" files it renames from don't count.
                var opts = new QueryOptions(CommonFileQuery.DefaultQuery, new[] { ".ini" });
                query = folder.CreateFileQueryWithOptions(opts);
                query.ContentsChanged += (s, a) => OnUi(Reload);
                await query.GetFilesAsync(); // events only start after the first GetFilesAsync
                WidgetLog.Line("load", "watching LocalState for crosshair.ini");
            }
            catch (Exception ex)
            {
                query = null; // then we only reload on activation / visibility changes
                WidgetLog.Line("error", "can't watch LocalState, reloading only when Game Bar opens - " + WidgetLog.Error(ex));
            }
        }

        /// <summary>Reads the ini (and the pictures if seq changed). Overlapping calls run once more at the end.</summary>
        async void Reload()
        {
            if (loading) { loadAgain = true; return; }
            loading = true;
            try
            {
                do
                {
                    loadAgain = false;
                    await LoadOnce();
                } while (loadAgain);
            }
            finally
            {
                loading = false;
            }
        }

        async Task LoadOnce()
        {
            string text = null;
            bool missing = false;
            Exception err = null;
            for (int tries = 0; tries < 3 && text == null; tries++)
            {
                try
                {
                    var file = await folder.TryGetItemAsync("crosshair.ini") as StorageFile;
                    if (file == null) { missing = true; break; }
                    text = await FileIO.ReadTextAsync(file);
                }
                catch (Exception ex)
                {
                    err = ex;
                    await Task.Delay(30); // the app was just renaming a new file over it
                }
            }
            if (text == null)
            {
                WidgetLog.Line("ini", missing
                    ? "no crosshair.ini - Aimdot isn't in Game Bar mode yet (or isn't running)"
                    : "can't read crosshair.ini after 3 tries - " + WidgetLog.Error(err));
                state = new WidgetState();
                ShowPicture();
                Chrome();
                return;
            }
            var s = WidgetState.Parse(text);
            if (s.Seq != seq)
            {
                WidgetLog.Line("ini", "seq " + s.Seq + " format " + s.Format + " visible " + (s.Visible ? 1 : 0) +
                    " offset " + s.OffsetX + "," + s.OffsetY + " rainbow " + (s.Rainbow ? 1 : 0) +
                    (s.Format > 1 ? " - written by a newer Aimdot, update the widget" : ""));
                logPlace = true;
            }

            if (s.Recenter != recenter)
            {
                bool first = recenter == -1;
                recenter = s.Recenter;
                if (!first) await CenterAsync();
            }

            if (s.Seq != seq && s.Visible)
            {
                if (s.Rainbow)
                {
                    var b = await Picture.LoadAsync(folder, "crosshair-base.png", BitmapAlphaMode.Premultiplied);
                    var f = await Picture.LoadAsync(folder, "crosshair-fill.png", BitmapAlphaMode.Straight);
                    rainbow = Rainbow.Fits(b, f) ? new Rainbow(b, f, s) : null;
                    if (rainbow != null)
                    {
                        SetSize(rainbow.W, rainbow.H);
                        WidgetLog.Line("picture", "rainbow layers " + rainbow.W + "x" + rainbow.H);
                    }
                    else
                    {
                        WidgetLog.Line("picture", "rainbow layers " + (b == null ? "base missing" : f == null ? "fill missing" : "of different sizes") +
                            " - showing crosshair.png without the rainbow");
                    }
                }
                else
                {
                    rainbow = null;
                }
                if (rainbow == null)
                {
                    var p = await Picture.LoadAsync(folder, "crosshair.png", BitmapAlphaMode.Premultiplied);
                    if (p == null)
                    {
                        // Mid-write: try again a few times, then wait for the next change.
                        if (++misses <= 5) { loadAgain = true; await Task.Delay(50); }
                        else WidgetLog.Line("picture", "can't read crosshair.png (5 tries) - waiting for the next change");
                        return;
                    }
                    misses = 0;
                    SetSize(p.W, p.H);
                    WidgetLog.Line("picture", "crosshair.png " + p.W + "x" + p.H);
                    p.Bgra.CopyTo(0, bitmap.PixelBuffer, 0, p.Bgra.Length);
                    bitmap.Invalidate();
                }
            }
            seq = s.Seq;
            state = s;
            ShowPicture();
            Chrome();
        }

        void SetSize(int w, int h)
        {
            if (bitmap == null || w != pixW || h != pixH)
            {
                pixW = w; pixH = h;
                bitmap = new WriteableBitmap(w, h);
                frame = new byte[w * h * 4];
                Crosshair.Source = bitmap;
            }
        }

        void ShowPicture()
        {
            Crosshair.Visibility = state.Visible && bitmap != null ? Visibility.Visible : Visibility.Collapsed;
            Place();
            Animate();
        }

        // ---- Rainbow -----------------------------------------------------------------------------------------

        void Animate()
        {
            bool run = rainbow != null && state.Visible && (widget == null || widget.Visible);
            if (run && !anim.IsEnabled) { DrawRainbow(); anim.Start(); }
            else if (!run && anim.IsEnabled) anim.Stop();
        }

        void DrawRainbow()
        {
            if (rainbow == null || bitmap == null || frame == null) return;
            rainbow.Render(clock.Elapsed.TotalSeconds, frame);
            frame.CopyTo(0, bitmap.PixelBuffer, 0, frame.Length);
            bitmap.Invalidate();
        }

        // ---- Placement: centre of the screen + offset, on whole screen pixels -------------------------------------

        void Place()
        {
            if (pixW <= 0 || pixH <= 0) return;
            var di = DisplayInformation.GetForCurrentView();
            double scale = di.RawPixelsPerViewPixel > 0 ? di.RawPixelsPerViewPixel : 1.0;
            double cw = Root.ActualWidth, ch = Root.ActualHeight;   // our content, in view pixels
            double left, top;                                       // picture's top-left in our content, screen pixels
            Rect b = widget != null ? widget.WindowBounds : Rect.Empty;
            if (widget != null && !b.IsEmpty && b.Width > 0 && b.Height > 0 && cw > 0)
            {
                // The docs don't say which unit WindowBounds uses: take whichever matches our own width.
                double k = Math.Abs(b.Width - cw * scale) <= Math.Abs(b.Width - cw) ? 1.0 : scale;
                double bx = b.X * k, by = b.Y * k, bw = b.Width * k, bh = b.Height * k;
                // Our content sits centred left-right in those bounds and at the bottom (if the bounds count a title
                // bar, it's on top).
                double ox = bx + (bw - cw * scale) / 2;
                double oy = by + bh - ch * scale;
                // Same rounding as the normal overlay: centre (whole pixels) minus half the picture.
                double sx = Math.Floor(di.ScreenWidthInRawPixels / 2.0) - Math.Floor(pixW / 2.0) + state.OffsetX;
                double sy = Math.Floor(di.ScreenHeightInRawPixels / 2.0) - Math.Floor(pixH / 2.0) + state.OffsetY;
                left = sx - ox;
                top = sy - oy;
            }
            else
            {
                // Not in Game Bar (or no bounds yet): centre of our own window.
                left = Math.Floor(cw * scale / 2) - Math.Floor(pixW / 2.0) + state.OffsetX;
                top = Math.Floor(ch * scale / 2) - Math.Floor(pixH / 2.0) + state.OffsetY;
            }
            Crosshair.Width = pixW / scale;
            Crosshair.Height = pixH / scale;
            Canvas.SetLeft(Crosshair, Math.Round(left) / scale);
            Canvas.SetTop(Crosshair, Math.Round(top) / scale);
            if (logPlace)
            {
                // Not on every call (dragging the widget calls this for every move): after a new ini, pin, DPI or mode change.
                logPlace = false;
                var inv = System.Globalization.CultureInfo.InvariantCulture;
                string bounds = widget != null && !b.IsEmpty
                    ? "window " + b.X.ToString("0.#", inv) + "," + b.Y.ToString("0.#", inv) + " " + b.Width.ToString("0.#", inv) + "x" + b.Height.ToString("0.#", inv)
                    : "no Game Bar window bounds";
                WidgetLog.Line("place", pixW + "x" + pixH + " at " + Math.Round(left) + "," + Math.Round(top) + " in the widget (" + bounds +
                    ", content " + cw.ToString("0.#", inv) + "x" + ch.ToString("0.#", inv) + ", scale " + scale.ToString("0.##", inv) + ")");
            }
        }

        async Task CenterAsync()
        {
            if (widget == null) return;
            try
            {
                await widget.CenterWindowAsync();
                WidgetLog.Line("place", "asked Game Bar to centre the widget window");
            }
            catch (Exception ex)
            {
                WidgetLog.Line("error", "Game Bar couldn't centre the widget window - " + WidgetLog.Error(ex));
            }
            logPlace = true;
            Place();
        }

        // ---- Frame + hint, only while Game Bar is open ----------------------------------------------------------

        void Chrome()
        {
            bool open = widget == null || widget.GameBarDisplayMode == XboxGameBarDisplayMode.Foreground;
            string hint = null;
            if (widget == null)
                hint = "Aimdot Game Bar widget. Press Win+G and open it from the Widget menu.";
            else if (seq == -1 && !state.Visible)
                hint = "Open Aimdot and choose Game Bar mode.";
            else if (!widget.Pinned)
                hint = "Pin this widget (pin icon) so the crosshair stays when Game Bar closes.";
            else if (!widget.ClickThroughEnabled)
                hint = "Turn on click-through (mouse icon in the top bar) so your clicks reach the game.";
            else if (!state.Visible)
                hint = "The crosshair is hidden. Show it in Aimdot or with its hotkey.";
            if (hint != lastHint)
            {
                lastHint = hint;
                WidgetLog.Line("hint", hint ?? "(none - set up and showing)");
            }
            Outline.Visibility = open ? Visibility.Visible : Visibility.Collapsed;
            Hint.Text = hint ?? "";
            Hint.Visibility = open && hint != null ? Visibility.Visible : Visibility.Collapsed;
        }
    }
}
