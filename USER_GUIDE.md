# User guide

Start here to customise Claude Code Usage Monitor and adjust its everyday settings.
For installation and provider sign-in requirements, see the [README](README.md).
The instructions below use the app's English labels.

## Contents

- [Open the dashboard](#open-the-dashboard)
- [Duplicate and customise a built-in theme](#duplicate-and-customise-a-built-in-theme)
- [Choose providers and refresh usage](#choose-providers-and-refresh-usage)
- [Show used or remaining allowance](#show-used-or-remaining-allowance)

## Open the dashboard

With the default theme, right-click a provider icon in the Windows system tray
and choose **Open Dashboard**. If the icon is hidden, look in the tray's overflow
menu beside the clock.

You can also open the dashboard from PowerShell:

```powershell
claude-code-usage-monitor --dashboard
```

For a portable download, run `.\claude-code-usage-monitor.exe --dashboard` from
the folder containing the executable.

## Duplicate and customise a built-in theme

Built-in themes are read-only. Create an editable copy to make your own design;
the original stays available to switch back to.

### Create your copy

1. Open **Theme Studio** from the dashboard's navigation.
2. In the **Theme** dropdown at the top, select the theme you want to start from,
   such as **Compact Fluent Quad**. Built-in themes are marked **(built-in)**.
3. Click the toolbar's copy icon beside the theme dropdown. Hover over it to see
   the **Duplicate...** tooltip. Use this top toolbar control to copy the whole
   theme; the **Duplicate** control beneath the layer list copies a layer.
4. In **Duplicate theme**, enter a name such as `My theme`, then click
   **Create copy**.

The app saves your copy, selects it in Theme Studio, and makes it the active
theme. Its controls are now editable.

### Make and save a change

1. Select a layer in the tree on the left. Expand its arrow to find nested layers.
2. Use the inspector on the right to edit the selected layer. Start with a small
   change, such as a colour or text size, and check the preview in the centre.
   **Appearance** contains appearance controls; **Positioning** contains layout
   controls. The available fields depend on the selected layer.
3. Click the toolbar's disk icon, with the **Save** tooltip, to save your changes
   and apply them to the running widget.

**Live apply** is disabled by default. With it disabled, edits update the Studio
preview until you click **Save**. Enable it to save and apply each edit
automatically. Enabling it also saves any changes already waiting in the editor.

Use the toolbar's **Undo** and **Redo** icons, or **Ctrl+Z** and **Ctrl+Y**, to
reverse or restore edits. With Live apply disabled, save again to apply an undo
to the running widget. If you close the dashboard or switch themes with unsaved
changes, choose **Save and continue**, **Discard changes**, or **Cancel**.

### Switch back to a built-in theme

Select the original theme from the **Theme** dropdown in Theme Studio. You can
also choose it under **Settings > Appearance > Active theme**. Switching applies
the selected theme; your saved custom copy remains available in the dropdown.

Custom theme files are stored in:

```text
%APPDATA%\ClaudeCodeUsageMonitor\themes
```

## Choose providers and refresh usage

1. Sign in to the provider you want to monitor using its own app or CLI. See
   [Provider setup](README.md#provider-setup) for provider-specific requirements.
2. Open **Settings** and find **Providers**. Enable the providers you want to
   monitor and disable any you do not use.
3. Under **Settings > General**, set **Update frequency** to the number of
   minutes between usage refreshes. This controls usage polling, not checks for
   new app versions.
4. Click **Refresh now** beside that setting when you want a fresh reading
   without waiting for the next scheduled refresh.

These settings save automatically. The default theme adapts to enabled providers;
a custom theme must include layers for the providers you want to display.

## Show used or remaining allowance

1. Open **Settings > Display**.
2. Set **Usage direction** to **Used** or **Remaining**.

| Option | What the percentage means | Example |
| --- | --- | --- |
| **Used** (default) | How much of the allowance you have consumed. | After using 30%, the display reads 30%. |
| **Remaining** | How much allowance is left. | After using 30%, the display reads 70%. |

The setting saves automatically. The default theme and Compact Fluent Quad
support both directions. Custom themes need to support this setting too; a
theme that always displays consumed usage may stay unchanged. See the
[theme binding notes](README.md#usage) if you are editing usage expressions.
