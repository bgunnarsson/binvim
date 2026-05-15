-- Sample Lua module — exercises common constructs.

local M = {}

local function clamp(value, lo, hi)
    if value < lo then return lo end
    if value > hi then return hi end
    return value
end

---@class Vector
---@field x number
---@field y number
local Vector = {}
Vector.__index = Vector

function Vector.new(x, y)
    return setmetatable({ x = x or 0, y = y or 0 }, Vector)
end

function Vector:length()
    return math.sqrt(self.x * self.x + self.y * self.y)
end

function Vector:__tostring()
    return string.format("(%g, %g)", self.x, self.y)
end

function Vector.__add(a, b)
    return Vector.new(a.x + b.x, a.y + b.y)
end

function M.greet(name, opts)
    opts = opts or {}
    local prefix = opts.prefix or "Hello"
    return string.format("%s, %s!", prefix, name)
end

function M.iter_squares(n)
    local i = 0
    return function()
        i = i + 1
        if i > n then return nil end
        return i, i * i
    end
end

if ... == nil then
    print(M.greet("world"))
    print(M.greet("rust", { prefix = "Hi" }))

    local v = Vector.new(3, 4) + Vector.new(0, 0)
    print(tostring(v), "len =", v:length())

    for i, sq in M.iter_squares(5) do
        print(i, sq, "clamped:", clamp(sq, 0, 10))
    end
end

return M
