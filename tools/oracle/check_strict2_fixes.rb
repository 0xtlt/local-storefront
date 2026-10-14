# frozen_string_literal: true

# Checks the rewrites `lsf check --fix` makes for Shopify's strict parser against the reference
# Liquid gem. The input is a JSON array of {template, fixed, environment, filesystem}, as written
# by `STRICT2_SPEC_FIXES=<file> cargo test -p lsf-liquid --test strict2_spec`.
#
# For each entry, the original template is rendered in `:warn` mode, the mode Shopify renders
# themes in today, and the fixed template in `:strict2` mode, the strict parser. They must give
# the same output, and the fixed template must parse. Each is rendered twice: with the entry's
# data, and with data where every name the templates mention is a drop that prints its own
# path (`{{ a.b }}` prints `a.b`), so that reading a different variable shows.
#
# Run with `mise run oracle:strict2-fixes <file>`.

ENV["TZ"] = "UTC"
Encoding.default_external = Encoding::UTF_8
Encoding.default_internal = Encoding::UTF_8

require "json"
require "liquid"

class MemoryFileSystem
  def initialize(partials)
    @partials = partials || {}
  end

  def read_template_file(name)
    @partials.fetch(name) { @partials.fetch("#{name}.liquid") { raise Liquid::FileSystemError, "No such template '#{name}'" } }
  end
end

# Prints the path it was looked up by.
class PathDrop < Liquid::Drop
  def initialize(path)
    super()
    @path = path
  end

  def liquid_method_missing(name)
    PathDrop.new("#{@path}.#{name}")
  end

  def to_s
    @path
  end
end

def probe_environment(entry)
  # `self` is left alone: it stands for the variables themselves.
  names = (entry["template"] + entry["fixed"]).scan(/[A-Za-z_][\w-]*\??/).uniq - ["self"]
  names.to_h { |name| [name, PathDrop.new(name)] }.merge(entry["environment"] || {})
end

def render(source, entry, error_mode, environment)
  template = Liquid::Template.parse(source, error_mode: error_mode, line_numbers: true)
  output = template.render(
    environment,
    registers: { file_system: MemoryFileSystem.new(entry["filesystem"]) },
  )
  [:ok, output]
rescue Liquid::SyntaxError => e
  [:syntax, e.message]
rescue StandardError => e
  [:raised, "#{e.class}: #{e.message}"]
end

entries = JSON.parse(File.read(ARGV.fetch(0)))
# What `--fix-intent` writes changes the output on purpose: it only has to parse strictly.
parsed = entries.select { |entry| entry["meant"] }.count do |entry|
  Liquid::Template.parse(entry["meant"], error_mode: :strict2)
  true
rescue Liquid::SyntaxError => e
  puts "#{entry["name"]}: the intent does not parse in strict2 mode: #{e.message}"
  puts "  meant: #{entry["meant"].inspect}"
  false
end
intents = entries.count { |entry| entry["meant"] }
puts "#{parsed} of #{intents} intents parse in strict2 mode"
failures = 0
broken = 0
entries.each do |entry|
  [entry["environment"] || {}, probe_environment(entry)].each do |environment|
    today = render(entry["template"], entry, :warn, environment)
    # A template that does not parse today has nothing to keep.
    if today[0] == :syntax
      broken += 1
      break
    end

    strict = render(entry["fixed"], entry, :strict2, environment)
    next if strict == today
    # A fix that says it changes the output has to parse, not to render the same.
    next if entry["changes"] && strict[0] == :ok

    failures += 1
    puts "#{entry["name"]}:"
    puts "  template: #{entry["template"].inspect}"
    puts "  fixed:    #{entry["fixed"].inspect}"
    puts "  today:    #{today.inspect}"
    puts "  strict2:  #{strict.inspect}"
    break
  end
end
checked = entries.size - broken
puts "#{checked - failures} of #{checked} fixes render the same with the reference gem " \
  "(#{broken} templates do not parse today)"
exit(failures.zero? && parsed == intents ? 0 : 1)
