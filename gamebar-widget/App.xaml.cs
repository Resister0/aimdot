// Aimdot Game Bar widget - app entry. Activation follows Microsoft's WidgetSampleCS:
// https://learn.microsoft.com/en-us/gaming/game-bar/guide/app-activation
using System;
using Microsoft.Gaming.XboxGameBar;
using Windows.ApplicationModel;
using Windows.ApplicationModel.Activation;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;

namespace CrosshairWidget
{
    sealed partial class App : Application
    {
        // Must be kept alive for as long as the widget is open (it holds the connection to Game Bar).
        private XboxGameBarWidget widget = null;

        public App()
        {
            InitializeComponent();
            Suspending += OnSuspending;
        }

        protected override void OnActivated(IActivatedEventArgs args)
        {
            XboxGameBarWidgetActivatedEventArgs widgetArgs = null;
            if (args.Kind == ActivationKind.Protocol)
            {
                var protocolArgs = args as IProtocolActivatedEventArgs;
                if (protocolArgs != null && protocolArgs.Uri.Scheme.Equals("ms-gamebarwidget"))
                {
                    widgetArgs = args as XboxGameBarWidgetActivatedEventArgs;
                }
            }
            if (widgetArgs == null)
            {
                return;
            }
            // Only the first activation makes the widget; later ones (repeat activations from Game Bar) are ignored.
            if (widgetArgs.IsLaunchActivation)
            {
                var rootFrame = new Frame();
                Window.Current.Content = rootFrame;
                WidgetLog.Line("load", "activated by Game Bar");
                widget = new XboxGameBarWidget(widgetArgs, Window.Current.CoreWindow, rootFrame);
                rootFrame.Navigate(typeof(CrosshairPage), widget);
                Window.Current.Closed += WidgetWindow_Closed;
                Window.Current.Activate();
            }
        }

        private void WidgetWindow_Closed(object sender, Windows.UI.Core.CoreWindowEventArgs e)
        {
            WidgetLog.Line("load", "widget closed");
            widget = null;
            Window.Current.Closed -= WidgetWindow_Closed;
        }

        // Normally unreachable (AppListEntry="none" hides it from Start), but Visual Studio's F5 can launch it this
        // way: show the same page without Game Bar, which then just shows a hint.
        protected override void OnLaunched(LaunchActivatedEventArgs e)
        {
            var rootFrame = Window.Current.Content as Frame;
            if (rootFrame == null)
            {
                rootFrame = new Frame();
                Window.Current.Content = rootFrame;
            }
            if (!e.PrelaunchActivated)
            {
                WidgetLog.Line("load", "launched outside Game Bar");
                if (rootFrame.Content == null)
                {
                    rootFrame.Navigate(typeof(CrosshairPage), null);
                }
                Window.Current.Activate();
            }
        }

        // Game Bar never suspends an app it has a live widget connection to, so this means the widget is gone.
        private void OnSuspending(object sender, SuspendingEventArgs e)
        {
            var deferral = e.SuspendingOperation.GetDeferral();
            WidgetLog.Line("load", "suspended (no live Game Bar connection)");
            widget = null;
            deferral.Complete();
        }
    }
}
