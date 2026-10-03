# frozen_string_literal: true

# Renders every case in crates/liquid/tests/cases/*.txt with Shopify's reference Liquid gem and
# writes the results to crates/liquid/tests/golden/*.json. The Rust test suite then checks that
# our engine produces byte-identical output.
#
# Case file format, one template per line:
#   ## a comment
#   #! data {"json": "object"}        variables for the following templates
#   #! partial name template source   a snippet available to render/include
#   #! parse {"option": true}         parse options for the following templates
#   {{ template }} with \n and \t escapes
#
# Run with `mise run oracle:golden`.

ENV["TZ"] = "UTC"
Encoding.default_external = Encoding::UTF_8
Encoding.default_internal = Encoding::UTF_8

require "json"
require "liquid"

ROOT = File.expand_path("../..", __dir__)
CASES = File.join(ROOT, "crates/liquid/tests/cases")
GOLDEN = File.join(ROOT, "crates/liquid/tests/golden")

class MemoryFileSystem
  def initialize(partials)
    @partials = partials
  end

  def read_template_file(name)
    @partials.fetch(name) { raise Liquid::FileSystemError, "No such template '#{name}'" }
  end
end

def unescape(text)
  text.gsub(/\\[nt\\]/) { |match| { "\\n" => "\n", "\\t" => "\t", "\\\\" => "\\" }[match] }
end

def render(source, data, partials, error_mode, options)
  template = Liquid::Template.parse(source, line_numbers: true, error_mode: error_mode, **options)
  template.render(
    JSON.parse(JSON.generate(data)),
    registers: { file_system: MemoryFileSystem.new(partials) },
  )
rescue Liquid::SyntaxError => e
  "SYNTAX: #{e.message}"
rescue StandardError => e
  "RAISED: #{e.class}: #{e.message}"
end

Dir.glob(File.join(CASES, "*.txt")).sort.each do |path|
  data = {}
  partials = {}
  options = {}
  cases = []
  File.readlines(path, chomp: true).each_with_index do |line, index|
    next if line.strip.empty? || line.start_with?("##")

    if line.start_with?("#! data ")
      data = JSON.parse(line.delete_prefix("#! data "))
      next
    end
    if line.start_with?("#! parse ")
      options = JSON.parse(line.delete_prefix("#! parse "), symbolize_names: true)
      next
    end
    if line.start_with?("#! partial ")
      name, source = line.delete_prefix("#! partial ").split(" ", 2)
      partials[name] = unescape(source || "")
      next
    end

    source = unescape(line)
    # `:warn` is the mode Shopify runs themes in: parse strictly, and fall back to the lax
    # parser (recording a warning) when that fails.
    expected = render(source, data, partials, :warn, options)
    entry = {
      "line" => index + 1,
      "template" => source,
      "data" => data,
      "partials" => partials.dup,
      "expected" => expected,
    }
    entry["parse"] = options unless options.empty?
    lax = render(source, data, partials, :lax, options)
    entry["lax"] = lax if lax != expected
    cases << entry
  end
  name = File.basename(path, ".txt")
  File.write(File.join(GOLDEN, "#{name}.json"), JSON.pretty_generate({ "liquid" => Liquid::VERSION, "cases" => cases }) + "\n")
  puts "#{name}: #{cases.size} cases"
end
