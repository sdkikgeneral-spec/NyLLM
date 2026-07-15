project(
  'winny_semantic_cache_poc',
  'cpp',
  version : '0.1.0',
  default_options : [
    'cpp_std=c++17',
    'warning_level=3',   # MSVC: /W4 相当 / GCC・Clang: -Wall -Wextra 相当
  ],
)

cpp = meson.get_compiler('cpp')

# インクルードルート(main.cpp は "src/..." "vendor/..." を poc/ 基準で参照)
inc = include_directories('.')

extra_args = []
# UTF-8ソース/実行文字セット(日本語リテラルのため MSVC で必須)
if cpp.get_id() == 'msvc'
  extra_args += ['/utf-8']
endif

deps = []

# --- オプション(既定はすべて無効 = 外部依存ゼロでビルド可能) ---
if get_option('use_sodium')
  deps += dependency('libsodium')
  extra_args += ['-DPOC_USE_SODIUM']
endif

if get_option('use_onnx')
  # onnxruntime の pkg-config 名。環境により 'libonnxruntime' の場合あり
  deps += dependency('onnxruntime')
  extra_args += ['-DPOC_USE_ONNX']
endif

executable(
  'semantic_cache_poc',
  'src/main.cpp',
  include_directories : inc,
  cpp_args : extra_args,
  dependencies : deps,
  install : false,
)
