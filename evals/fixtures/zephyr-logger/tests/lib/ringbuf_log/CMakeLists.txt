cmake_minimum_required(VERSION 3.20.0)

# Build this checkout's module, never a copy found elsewhere: append the
# module root unless the caller already passed it, then check by path.
get_filename_component(ZL_MODULE_DIR ${CMAKE_CURRENT_LIST_DIR}/../../.. ABSOLUTE)
if(NOT ZL_MODULE_DIR IN_LIST ZEPHYR_EXTRA_MODULES)
  list(APPEND ZEPHYR_EXTRA_MODULES ${ZL_MODULE_DIR})
endif()

find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(ringbuf_log_test)

if(NOT ZEPHYR_ZLOGGER_MODULE_DIR STREQUAL ZL_MODULE_DIR)
  message(FATAL_ERROR "zlogger module resolved to '${ZEPHYR_ZLOGGER_MODULE_DIR}', expected '${ZL_MODULE_DIR}'")
endif()

target_sources(app PRIVATE src/main.c)
