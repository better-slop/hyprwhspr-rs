-- hyprwhspr Walker/Elephant Menu
-- Copy to: ~/.config/elephant/menus/hyprwhspr.lua
--
-- Displays recent voice transcriptions with click-to-copy.
-- Invoke with: walker --provider menus:hyprwhspr

Name = "hyprwhspr"
NamePretty = "Recent Transcriptions"
Icon = "audio-input-microphone"
Cache = false
HideFromProviderlist = false
FixedOrder = true
Description = "Recent voice transcriptions"

function GetEntries()
    local entries = {}
    local home = os.getenv("HOME") or ""
    local data_home = os.getenv("XDG_DATA_HOME")
    if not data_home or data_home == "" then
        data_home = home .. "/.local/share"
    end
    local file_path = data_home .. "/hyprwhspr-rs/transcriptions.json"

    local file = io.open(file_path, "r")
    if not file then
        return entries
    end

    local content = file:read("*a")
    file:close()

    if not content or content == "" then
        return entries
    end

    local data = jsonDecode(content)
    if not data then
        return entries
    end

    for _, item in ipairs(data) do
        local display = item.text or ""
        if #display > 60 then
            display = string.sub(display, 1, 57) .. "..."
        end

        table.insert(entries, {
            Text = display,
            Subtext = item.timestamp or "",
            Value = item.text or "",
            Icon = "edit-copy",
            Preview = item.text or "",
            PreviewType = "text",
        })
    end

    return entries
end

-- Copy selected entry to clipboard.
-- Elephant pipes `Value` to stdin when no %VALUE%.
Action = "wl-copy"
