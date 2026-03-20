<!--
  - Copyright © 2026 Clyso GmbH
  -
  -  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
  -  you may not use this file except in compliance with the License.
  -  You may obtain a copy of the License at
  -
  -  https://www.gnu.org/licenses/agpl-3.0.html
  -
  -  Unless required by applicable law or agreed to in writing, software
  -  distributed under the License is distributed on an "AS IS" BASIS,
  -  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
  -  See the License for the specific language governing permissions and
  -  limitations under the License.
  -->

<script setup lang="ts">
  import { storeToRefs } from 'pinia';
  import {
    CAvatar,
    CColorSchemeToggle,
    CDashboardHeader,
    CDashboardLayout,
  } from '@clyso/clyso-ui-kit';
  import { onBeforeMount } from 'vue';
  import { useColorSchemeStore } from '@/stores/colorSchemeStore';
  import DashboardFooter from '@/components/DashboardFooter.vue';
  import DashboardLogo from '@/components/DashboardLogo.vue';
  import DashboardNav from '@/components/DashboardNav.vue';
  import { useAuthStore } from '@/stores/auth';

  const { colorScheme } = storeToRefs(useColorSchemeStore());
  const { setColorScheme } = useColorSchemeStore();

  const authStore = useAuthStore();
  const { user } = storeToRefs(authStore);

  onBeforeMount(() => authStore.fetchUser());
</script>

<template>
  <CDashboardLayout
    class="dashboard-view"
    :has-side-menu="false"
  >
    <template #header>
      <CDashboardHeader
        :options="[]"
        :has-side-menu="false"
        :has-user-menu="false"
      >
        <template #start>
          <DashboardLogo />
        </template>

        <template #end>
          <div class="dashboard-view__header-actions">
            <div
              class="dashboard-view__user-profile"
              v-if="user"
            >
              <CAvatar
                class="dashboard-view__user-profile"
                round
                size="large"
                :name="user.name"
                v-if="user"
              >
              </CAvatar>
            </div>
            <CColorSchemeToggle
              :value="colorScheme"
              @update:value="setColorScheme"
            />
          </div>
        </template>
      </CDashboardHeader>
    </template>

    <main class="dashboard-view__main">
      <DashboardNav class="dashboard-view__nav" />
      <div class="dashboard-view__render-view">
        <RouterView />
      </div>
    </main>

    <template #footer>
      <DashboardFooter />
    </template>
  </CDashboardLayout>
</template>

<style lang="scss" scoped>
  @use '@/styles/utils' as utils;

  .dashboard-view {
    &__main {
      height: 100%;
      max-width: 100vw;
      padding: utils.unit(5) utils.unit(8) utils.unit(12);
      display: grid;
    }

    &__nav {
      margin-bottom: utils.unit(8);
      min-width: 0;
    }

    &__header-actions {
      display: flex;
      align-items: center;
      gap: utils.unit(2);
    }

    &__user-profile {
      position: relative;
    }

    ::v-deep(.c-dashboard-header) {
      z-index: 2;
    }

    ::v-deep(.c-dashboard-layout__footer) {
      padding: 0;
    }
  }
</style>
