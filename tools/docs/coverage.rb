# frozen_string_literal: true
# Lists the properties of Shopify's Liquid objects that lsf never names, from the reference
# `mise run docs:fetch` downloads. A property counts as known when its name appears as a
# string anywhere in the code, so a miss is certain and a match is not: this is a list of
# leads, not a verdict. Properties a theme of `themes/` reads come first, with the count of
# files that read them. Usage: ruby tools/docs/coverage.rb
require "json"
Encoding.default_external = Encoding::UTF_8

root = File.expand_path("../..", __dir__)
reference = File.join(root, ".cache/theme-liquid-docs/data/objects.json")
abort "#{reference} is missing: run `mise run docs:fetch`" unless File.file?(reference)

known = Dir.glob(File.join(root, "crates/*/src/**/*.rs"))
           .flat_map { |path| File.read(path).scan(/"([a-z0-9_?]+)"/).flatten }
           .to_set

# The Liquid code of a file: what is inside `{{ }}` and `{% %}`, without its quoted strings
# (translation keys such as `'actions.header'` are not properties).
def liquid_code(source)
  source.scan(/\{\{.*?\}\}|\{%.*?%\}/m).join("\n").gsub(/'[^']*'|"[^"]*"/, "''")
end

themes = Dir.glob(File.join(root, "themes/*/")).to_h do |theme|
  files = Dir.glob(File.join(theme, "**/*.liquid")).map { |path| liquid_code(File.read(path)) }
  [File.basename(theme), files]
end

leads = JSON.parse(File.read(reference)).reject { |object| object["deprecated"] }.flat_map do |object|
  (object["properties"] || []).reject { |property| property["deprecated"] || known.include?(property["name"]) }
                              .map do |property|
    # `object.property` or `list[0].property`.
    pattern = /(?<=[a-z0-9_\]])\.#{Regexp.escape(property["name"])}(?![a-z0-9_?])/
    uses = themes.transform_values { |files| files.count { |source| source.match?(pattern) } }
                 .select { |_, count| count.positive? }
    { object: object["name"], property: property["name"], uses: uses }
  end
end

if leads.empty?
  puts "Every documented property is named somewhere in lsf."
  exit
end

used, unused = leads.partition { |lead| lead[:uses].any? }
puts "Properties lsf never names (#{leads.size}):"
unless used.empty?
  puts "\nRead by a theme of themes/:"
  used.each do |lead|
    puts "  #{lead[:object]}.#{lead[:property]}  (#{lead[:uses].map { |theme, count| "#{theme}: #{count}" }.join(', ')})"
  end
end
unless unused.empty?
  puts "\n#{themes.empty? ? 'All' : 'Read by no theme of themes/'}:"
  unused.group_by { |lead| lead[:object] }.each do |object, group|
    puts "  #{object}: #{group.map { |lead| lead[:property] }.join(', ')}"
  end
end
