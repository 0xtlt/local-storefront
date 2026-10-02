# frozen_string_literal: true

# Turns the YAML specs of .cache/liquid-spec into JSON, one file per suite, in
# .cache/liquid-spec-cases/. The Rust tests read those files, so they need neither Ruby nor a
# YAML parser, and every scalar is resolved the way Ruby resolves it for the reference runner.
#
# The suite's own loader does the reading: it applies the suite and file defaults (error mode,
# inline errors, features, shared data files) exactly like `liquid-spec run` does.
#
# JSON has no symbol, time, range, regexp or byte string, its object keys are strings and it
# cannot hold a structure that contains itself. Those values are written as `{"$type": ...}`
# objects; see `encode`.
#
# Run with `mise run liquid-spec:fetch`.

Encoding.default_external = Encoding::UTF_8
Encoding.default_internal = Encoding::UTF_8

require "date"
require "fileutils"
require "json"
require "time"

ROOT = File.expand_path("../..", __dir__)
SUITE = File.join(ROOT, ".cache/liquid-spec")
OUT = File.join(ROOT, ".cache/liquid-spec-cases")

$LOAD_PATH.unshift(File.join(SUITE, "lib"))
require "liquid/spec/spec_loader"
require "liquid/spec/suite"

# Some specs draw random numbers into their template and expected output. A fixed seed makes
# two exports of the same revision identical.
SEED = 20_240_101

def encode(value, parents = [])
  if value.is_a?(Array) || value.is_a?(Hash)
    return { "$type" => "cycle" } if parents.any? { |parent| parent.equal?(value) }

    parents += [value]
  end
  case value
  when nil, true, false
    value
  when String
    text = value.dup.force_encoding(Encoding::UTF_8)
    text.valid_encoding? ? text : { "$type" => "binary", "base64" => [value].pack("m0") }
  when Integer
    value.bit_length < 64 ? value : { "$type" => "integer", "value" => value.to_s }
  when Float
    value.finite? ? value : { "$type" => "float", "value" => value.to_s }
  when Symbol
    { "$type" => "symbol", "value" => value.to_s }
  when Range
    { "$type" => "range", "begin" => encode(value.begin), "end" => encode(value.end), "exclude_end" => value.exclude_end? }
  when Regexp
    { "$type" => "regexp", "source" => value.source, "ignore_case" => value.casefold? }
  when Time, DateTime
    time = value.to_time
    { "$type" => "time", "value" => time.iso8601(9), "to_s" => value.to_s }
  when Date
    { "$type" => "date", "value" => value.iso8601 }
  when Array
    value.map { |item| encode(item, parents) }
  when Hash
    if value.keys.all? { |key| key.is_a?(String) && encode(key).is_a?(String) }
      value.to_h { |key, item| [encode(key), encode(item, parents)] }
    else
      { "$type" => "hash", "pairs" => value.map { |key, item| [encode(key, parents), encode(item, parents)] } }
    end
  else
    { "$type" => "unknown", "class" => value.class.name, "to_s" => value.to_s }
  end
end

def export(spec)
  {
    "name" => spec.name,
    "file" => spec.source_file.delete_prefix("#{SUITE}/specs/"),
    "line" => spec.line_number,
    "template" => encode(spec.template),
    "template_name" => spec.template_name,
    "expected" => encode(spec.expected),
    "expected_pattern" => encode(spec.expected_pattern),
    "errors" => encode(spec.errors),
    "environment" => encode(spec.raw_environment),
    "filesystem" => encode(spec.raw_filesystem),
    "template_factory" => encode(spec.raw_template_factory),
    "resource_limits" => encode(spec.raw_resource_limits),
    "error_modes" => spec.error_modes.map(&:to_s),
    "render_errors" => spec.render_errors ? true : false,
    "features" => spec.features.map(&:to_s),
    "complexity" => spec.complexity,
  }.compact
end

revision = `git -C #{SUITE} rev-parse HEAD`.strip
abort("#{SUITE} is not a checkout of liquid-spec") if revision.empty?

FileUtils.rm_rf(OUT)
FileUtils.mkdir_p(OUT)
Liquid::Spec::Suite.all.sort_by { |suite| suite.id.to_s }.each do |suite|
  srand(SEED)
  specs = Liquid::Spec::SpecLoader.load_suite(suite).sort_by { |spec| [spec.source_file, spec.line_number || 0] }
  document = { "suite" => suite.id.to_s, "revision" => revision, "specs" => specs.map { |spec| export(spec) } }
  File.write(File.join(OUT, "#{suite.id}.json"), JSON.generate(document))
  puts "#{suite.id}: #{specs.size} specs"
end
File.write(File.join(OUT, "REVISION"), "#{revision}\n")
