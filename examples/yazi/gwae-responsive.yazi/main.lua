--- @since 26.8.15
--- @sync entry

local M = {}

function M:setup(opts)
	-- Gwae sets this for every child pane. Leave standalone Yazi untouched.
	if not os.getenv("GWAE_PANE") or self._installed then
		return
	end
	opts = opts or {}
	local preview_width = opts.preview_width or 64
	local parent_width = opts.parent_width or 96
	assert(preview_width > 0 and parent_width > preview_width, "expected 0 < preview_width < parent_width")

	-- Snapshot the user's full layout before narrowing it. Never derive the
	-- next layout from a ratio we already collapsed, or panels stay hidden.
	local r = rt.mgr.ratio
	local full = { r[1], r[2], r[3] }
	local layout = Tab.layout
	Tab.layout = function(tab)
		local width = tab._area.w
		if width < preview_width then
			rt.mgr.ratio = { 0, 1, 0 }
		elseif width < parent_width then
			rt.mgr.ratio = { 0, math.max(1, full[2]), full[3] }
		else
			rt.mgr.ratio = { full[1], full[2], full[3] }
		end
		-- Keep Yazi's own layout, padding, rails, hit testing, and preview
		-- geometry. This runs on resize as well as initial construction.
		return layout(tab)
	end
	self._installed = true
end

-- Also allow enabling in an existing session with `plugin gwae-responsive`.
function M:entry()
	self:setup()
	ya.emit("app:resize", {})
end

return M
