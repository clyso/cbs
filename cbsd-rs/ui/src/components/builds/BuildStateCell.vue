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
  import { CTag } from '@clyso/clyso-ui-kit';
  import { computed } from 'vue';
  import { BuildStateHelper } from '@/utils/helpers/buildStateHelper';
  import { type Build } from '@/utils/types/cbs';

  const { build } = defineProps<{
    build: Build;
  }>();

  type stateType = 'default' | 'primary' | 'success' | 'warning' | 'error';

  const stateString = computed<string>(() => {
    if (!build.state) return 'Unknown';

    return BuildStateHelper.toSentenceCase(build.state);
  });

  const stateColor = computed<stateType>(() => {
    if (BuildStateHelper.isNeutralBuildState(build.state)) return 'primary';

    if (BuildStateHelper.isSuccessBuildState(build.state)) return 'success';

    if (BuildStateHelper.isWarningBuildState(build.state)) return 'warning';

    if (BuildStateHelper.isErrorBuildState(build.state)) return 'error';

    return 'default';
  });
</script>

<template>
  <div class="build-state-cell">
    <CTag
      :bordered="false"
      round
      class="build-state-cell__state"
      :type="stateColor"
    >
      {{ stateString }}
    </CTag>
  </div>
</template>
