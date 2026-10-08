cmake_minimum_required(VERSION 3.20.0)

get_filename_component(ZL_MODULE_DIR ${CMAKE_CURRENT_LIST_DIR}/.. ABSOLUTE)
if(NOT ZL_MODULE_DIR IN_LIST ZEPHYR_EXTRA_MODULES)
  list(APPEND ZEPHYR_EXTRA_MODULES ${ZL_MODULE_DIR})
endif()

find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(zlogger_app)

target_sources(app PRIVATE src/main.c)
